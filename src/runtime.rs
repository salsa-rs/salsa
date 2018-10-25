use crate::{Database, SweepStrategy};
use lock_api::RawRwLock;
use log::debug;
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockUpgradableReadGuard};
use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;
use std::cell::RefCell;
use std::fmt::Write;
use std::hash::BuildHasherDefault;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub(crate) type FxIndexSet<K> = indexmap::IndexSet<K, BuildHasherDefault<FxHasher>>;

/// The salsa runtime stores the storage for all queries as well as
/// tracking the query stack and dependencies between cycles.
///
/// Each new runtime you create (e.g., via `Runtime::new` or
/// `Runtime::default`) will have an independent set of query storage
/// associated with it. Normally, therefore, you only do this once, at
/// the start of your application.
pub struct Runtime<DB: Database> {
    id: RuntimeId,
    shared_state: Arc<SharedState<DB>>,
    local_state: RefCell<LocalState<DB>>,
}

impl<DB> Default for Runtime<DB>
where
    DB: Database,
{
    fn default() -> Self {
        Runtime {
            id: RuntimeId { counter: 0 },
            shared_state: Default::default(),
            local_state: Default::default(),
        }
    }
}

impl<DB> std::fmt::Debug for Runtime<DB>
where
    DB: Database,
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Runtime")
            .field("id", &self.id())
            .field("revision", &self.current_revision())
            .finish()
    }
}

impl<DB> Runtime<DB>
where
    DB: Database,
{
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the underlying storage, where the keys/values for all queries are kept.
    pub fn storage(&self) -> &DB::DatabaseStorage {
        &self.shared_state.storage
    }

    /// As with `Database::fork`, creates a second copy of the runtime
    /// meant to be used from another thread.
    ///
    /// **Warning.** This second handle is intended to be used from a
    /// separate thread. Using two database handles from the **same
    /// thread** can lead to deadlock.
    pub fn fork(&self) -> Self {
        Runtime {
            id: RuntimeId {
                counter: self.shared_state.next_id.fetch_add(1, Ordering::SeqCst),
            },
            shared_state: self.shared_state.clone(),
            local_state: Default::default(),
        }
    }

    /// Indicates that some input to the system has changed and hence
    /// that memoized values **may** be invalidated. This cannot be
    /// invoked while query computation is in progress.
    ///
    /// As a user of the system, you would not normally invoke this
    /// method directly. Instead, you would use "input" queries and
    /// invoke their `set` method. But it can be useful if you have a
    /// "volatile" input that you must poll from time to time; in that
    /// case, you can wrap the input with a "no-storage" query and
    /// invoke this method from time to time.
    pub fn next_revision(&self) {
        self.increment_revision();
    }

    /// Default implementation for `Database::sweep_all`.
    pub fn sweep_all(&self, db: &DB, strategy: SweepStrategy) {
        // Note that we do not acquire the query lock (or any locks)
        // here.  Each table is capable of sweeping itself atomically
        // and there is no need to bring things to a halt. That said,
        // users may wish to guarantee atomicity.

        db.for_each_query(|query_storage| query_storage.sweep(db, strategy));
    }

    /// Indicates that a derived query has begun to execute; if this is the
    /// first derived query on this thread, then acquires a read-lock on the
    /// runtime to prevent us from moving to a new revision until that query
    /// completes.
    ///
    /// (However, if other threads invoke `increment_revision`, then
    /// the current revision may be considered cancelled, which can be
    /// observed through `is_current_revision_canceled`.)
    pub(crate) fn start_query(&self) -> Option<QueryGuard<'_, DB>> {
        let mut local_state = self.local_state.borrow_mut();
        if !local_state.query_in_progress {
            local_state.query_in_progress = true;
            let guard = self.shared_state.query_lock.read();

            Some(QueryGuard::new(self, guard))
        } else {
            None
        }
    }

    /// Locks the current revision and returns a guard object that --
    /// when dropped -- will unlock it. While a revision is locked,
    /// queries can execute as normal but calls to `set` will block
    /// (note that calls to `set` *do* set the cancellation flag,
    /// which you can can check with
    /// `is_current_revision_canceled`). The intention is that you can
    /// lock the revision and then do multiple queries, thus
    /// guaranteeing that all of those queries execute against a
    /// consistent "view" of the database.
    ///
    /// Note that, unlike most RAII guards, the guard returned by this
    /// method does not borrow the database or the runtime
    /// (internally, it uses an `Arc` handle). This means it can be
    /// sent to other threads without a problem -- the lock persists
    /// as long as the guard has not yet been dropped.
    ///
    /// ### Deadlock warning
    ///
    /// If you invoke `lock_revision` and then, from the same thread,
    /// call `set` on some input, you will get a deadlock.
    pub fn lock_revision(&self) -> RevisionGuard<DB> {
        RevisionGuard::new(&self.shared_state)
    }

    #[inline]
    pub(crate) fn id(&self) -> RuntimeId {
        self.id
    }

    /// Read current value of the revision counter.
    #[inline]
    pub(crate) fn current_revision(&self) -> Revision {
        Revision {
            generation: self.shared_state.revision.load(Ordering::SeqCst) as u64,
        }
    }

    /// Check if the current revision is canceled. If this method ever
    /// returns true, the currently executing query is also marked as
    /// having an *untracked read* -- this means that, in the next
    /// revision, we will always recompute its value "as if" some
    /// input had changed. This means that, if your revision is
    /// canceled (which indicates that current query results will be
    /// ignored) your query is free to shortcircuit and return
    /// whatever it likes.
    #[inline]
    pub fn is_current_revision_canceled(&self) -> bool {
        let pending_revision_increments = self
            .shared_state
            .pending_revision_increments
            .load(Ordering::SeqCst);
        if pending_revision_increments > 0 {
            self.report_untracked_read();
            true
        } else {
            false
        }
    }

    /// Increments the current revision counter and returns the new value.
    pub(crate) fn increment_revision(&self) -> Revision {
        log::debug!("increment_revision()");

        if self.query_in_progress() {
            panic!("increment_revision invoked during a query computation");
        }

        // Get an (upgradable) read lock, so that we are sure nobody
        // else is changing the current revision.
        let lock = self.shared_state.query_lock.upgradable_read();

        // Flag current revision as cancelled.
        // `increment_revision` calls, they may all set the
        let old_pending_revision_increments = self
            .shared_state
            .pending_revision_increments
            .fetch_add(1, Ordering::SeqCst);
        assert!(
            old_pending_revision_increments != usize::max_value(),
            "pending increment overflow"
        );

        // To modify the revision, we need the lock.
        let _lock = RwLockUpgradableReadGuard::upgrade(lock);

        // *Before* updating the revision number, reset
        // `revision_cancelled` to false.  This way, if anybody should
        // happen to invoke `is_current_revision_canceled` before we
        // update the number, they don't get an incorrect result (but
        // note that, because we hold `query_lock`, no queries can
        // be currently executing anyhow, so it's sort of a moot
        // point).
        self.shared_state
            .pending_revision_increments
            .fetch_sub(1, Ordering::SeqCst);

        let old_revision = self.shared_state.revision.fetch_add(1, Ordering::SeqCst);
        assert!(old_revision != usize::max_value(), "revision overflow");

        let result = Revision {
            generation: 1 + old_revision as u64,
        };

        debug!("increment_revision: incremented to {:?}", result);

        result
    }

    pub(crate) fn query_in_progress(&self) -> bool {
        self.local_state.borrow().query_in_progress
    }

    pub(crate) fn execute_query_implementation<V>(
        &self,
        descriptor: &DB::QueryDescriptor,
        execute: impl FnOnce() -> V,
    ) -> ComputedQueryResult<DB, V> {
        debug!("{:?}: execute_query_implementation invoked", descriptor);

        // Push the active query onto the stack.
        let push_len = {
            let mut local_state = self.local_state.borrow_mut();
            local_state
                .query_stack
                .push(ActiveQuery::new(descriptor.clone()));
            local_state.query_stack.len()
        };

        // Execute user's code, accumulating inputs etc.
        let value = execute();

        // Extract accumulated inputs.
        let ActiveQuery {
            subqueries,
            changed_at,
            ..
        } = {
            let mut local_state = self.local_state.borrow_mut();

            // Sanity check: pushes and pops should be balanced.
            assert_eq!(local_state.query_stack.len(), push_len);

            local_state.query_stack.pop().unwrap()
        };

        ComputedQueryResult {
            value,
            changed_at,
            subqueries,
        }
    }

    /// Reports that the currently active query read the result from
    /// another query.
    ///
    /// # Parameters
    ///
    /// - `descriptor`: the query whose result was read
    /// - `changed_revision`: the last revision in which the result of that
    ///   query had changed
    pub(crate) fn report_query_read(
        &self,
        descriptor: &DB::QueryDescriptor,
        changed_at: ChangedAt,
    ) {
        if let Some(top_query) = self.local_state.borrow_mut().query_stack.last_mut() {
            top_query.add_read(descriptor, changed_at);
        }
    }

    pub(crate) fn report_untracked_read(&self) {
        if let Some(top_query) = self.local_state.borrow_mut().query_stack.last_mut() {
            top_query.add_untracked_read(self.current_revision());
        }
    }

    /// Obviously, this should be user configurable at some point.
    pub(crate) fn report_unexpected_cycle(&self, descriptor: DB::QueryDescriptor) -> ! {
        debug!("report_unexpected_cycle(descriptor={:?})", descriptor);

        let local_state = self.local_state.borrow();
        let LocalState { query_stack, .. } = &*local_state;

        let start_index = (0..query_stack.len())
            .rev()
            .filter(|&i| query_stack[i].descriptor == descriptor)
            .next()
            .unwrap();

        let mut message = format!("Internal error, cycle detected:\n");
        for active_query in &query_stack[start_index..] {
            writeln!(message, "- {:?}\n", active_query.descriptor).unwrap();
        }
        panic!(message)
    }

    /// Try to make this runtime blocked on `other_id`. Returns true
    /// upon success or false if `other_id` is already blocked on us.
    pub(crate) fn try_block_on(
        &self,
        descriptor: &DB::QueryDescriptor,
        other_id: RuntimeId,
    ) -> bool {
        self.shared_state
            .dependency_graph
            .lock()
            .add_edge(self.id(), descriptor, other_id)
    }

    pub(crate) fn unblock_queries_blocked_on_self(&self, descriptor: &DB::QueryDescriptor) {
        self.shared_state
            .dependency_graph
            .lock()
            .remove_edge(descriptor, self.id())
    }
}

/// State that will be common to all threads (when we support multiple threads)
struct SharedState<DB: Database> {
    storage: DB::DatabaseStorage,

    /// Stores the next id to use for a forked runtime (starts at 1).
    next_id: AtomicUsize,

    /// Whenever derived queries are executing, they acquire this lock
    /// in read mode. Mutating inputs (and thus creating a new
    /// revision) requires a write lock (thus guaranteeing that no
    /// derived queries are in progress). Note that this is not needed
    /// to prevent **race conditions** -- the revision counter itself
    /// is stored in an `AtomicUsize` so it can be cheaply read
    /// without acquiring the lock.  Rather, the `query_lock` is used
    /// to ensure a higher-level consistency property.
    query_lock: RwLock<()>,

    /// Stores the current revision. This is an `AtomicUsize` because
    /// it may be *read* at any point without holding the
    /// `query_lock`. Updates, however, require the `query_lock` to be
    /// acquired. (See `query_lock` for details.)
    ///
    /// (Ideally, this should be `AtomicU64`, but that is currently unstable.)
    revision: AtomicUsize,

    /// Counts the number of pending increments to the revision
    /// counter. If this is non-zero, it means that the current
    /// revision is out of date, and hence queries are free to
    /// "short-circuit" their results if they learn that. See
    /// `is_current_revision_canceled` for more information.
    pending_revision_increments: AtomicUsize,

    /// The dependency graph tracks which runtimes are blocked on one
    /// another, waiting for queries to terminate.
    dependency_graph: Mutex<DependencyGraph<DB>>,
}

impl<DB: Database> Default for SharedState<DB> {
    fn default() -> Self {
        SharedState {
            next_id: AtomicUsize::new(1),
            storage: Default::default(),
            query_lock: Default::default(),
            revision: Default::default(),
            dependency_graph: Default::default(),
            pending_revision_increments: Default::default(),
        }
    }
}

/// State that will be specific to a single execution threads (when we
/// support multiple threads)
struct LocalState<DB: Database> {
    query_in_progress: bool,
    query_stack: Vec<ActiveQuery<DB>>,
}

impl<DB: Database> Default for LocalState<DB> {
    fn default() -> Self {
        LocalState {
            query_in_progress: false,
            query_stack: Default::default(),
        }
    }
}

pub(crate) struct QueryGuard<'db, DB: Database + 'db> {
    db: &'db Runtime<DB>,
    lock: RwLockReadGuard<'db, ()>,
}

impl<'db, DB: Database> QueryGuard<'db, DB> {
    fn new(db: &'db Runtime<DB>, lock: RwLockReadGuard<'db, ()>) -> Self {
        Self { db, lock }
    }
}

impl<'db, DB: Database> Drop for QueryGuard<'db, DB> {
    fn drop(&mut self) {
        let mut local_state = self.db.local_state.borrow_mut();
        assert!(local_state.query_in_progress);
        local_state.query_in_progress = false;
    }
}

/// The guard returned by `lock_revision`. Once this guard is dropped,
/// the revision will be unlocked, and calls to `set` can proceed.
pub struct RevisionGuard<DB: Database> {
    shared_state: Arc<SharedState<DB>>,
}

impl<DB: Database> RevisionGuard<DB> {
    /// Creates a new revision guard, acquiring the query read-lock in the process.
    fn new(shared_state: &Arc<SharedState<DB>>) -> Self {
        // Acquire the read-lock without using RAII. This requires the
        // unsafe keyword because, if we were to unlock the lock this way,
        // we would screw up other people using the safe APIs.
        unsafe {
            shared_state.query_lock.raw().lock_shared();
        }

        Self {
            shared_state: shared_state.clone(),
        }
    }
}

impl<DB: Database> Drop for RevisionGuard<DB> {
    fn drop(&mut self) {
        // Release our read-lock without using RAII. As in `new`
        // above, this requires the unsafe keyword.
        unsafe {
            self.shared_state.query_lock.raw().unlock_shared();
        }
    }
}

struct ActiveQuery<DB: Database> {
    /// What query is executing
    descriptor: DB::QueryDescriptor,

    /// Maximum revision of all inputs thus far;
    /// we also track if all inputs have been constant.
    ///
    /// If we see an untracked input, this is not terribly relevant.
    changed_at: ChangedAt,

    /// Set of subqueries that were accessed thus far, or `None` if
    /// there was an untracked the read.
    subqueries: Option<FxIndexSet<DB::QueryDescriptor>>,
}

pub(crate) struct ComputedQueryResult<DB: Database, V> {
    /// Final value produced
    pub(crate) value: V,

    /// Maximum revision of all inputs observed; `is_constant` is true
    /// if all inputs were constants.
    ///
    /// If we observe an untracked read, this will be set to a
    /// non-constant value that changed in the most recent revision.
    pub(crate) changed_at: ChangedAt,

    /// Complete set of subqueries that were accessed, or `None` if
    /// there was an untracked the read.
    pub(crate) subqueries: Option<FxIndexSet<DB::QueryDescriptor>>,
}

impl<DB: Database> ActiveQuery<DB> {
    fn new(descriptor: DB::QueryDescriptor) -> Self {
        ActiveQuery {
            descriptor,
            changed_at: ChangedAt {
                is_constant: true,
                revision: Revision::ZERO,
            },
            subqueries: Some(FxIndexSet::default()),
        }
    }

    fn add_read(&mut self, subquery: &DB::QueryDescriptor, changed_at: ChangedAt) {
        let ChangedAt {
            is_constant,
            revision,
        } = changed_at;

        if let Some(set) = &mut self.subqueries {
            set.insert(subquery.clone());
        }

        self.changed_at.is_constant &= is_constant;
        self.changed_at.revision = self.changed_at.revision.max(revision);
    }

    fn add_untracked_read(&mut self, changed_at: Revision) {
        self.subqueries = None;
        self.changed_at.is_constant = false;
        self.changed_at.revision = changed_at;
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RuntimeId {
    counter: usize,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision {
    generation: u64,
}

impl Revision {
    pub(crate) const ZERO: Self = Revision { generation: 0 };
}

impl std::fmt::Debug for Revision {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "R{}", self.generation)
    }
}

/// Records when a stamped value changed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ChangedAt {
    // Will this value ever change again?
    pub(crate) is_constant: bool,

    // At which revision did this value last change? (If this value is
    // the value of a constant input, this indicates when it became
    // constant.)
    pub(crate) revision: Revision,
}

impl ChangedAt {
    /// True if a value is stored with this `ChangedAt` value has
    /// changed after `revision`. This is invoked by query storage
    /// when their dependents are asking them if they have changed.
    pub(crate) fn changed_since(self, revision: Revision) -> bool {
        self.revision > revision
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StampedValue<V> {
    pub(crate) value: V,
    pub(crate) changed_at: ChangedAt,
}

struct DependencyGraph<DB: Database> {
    /// A `(K -> V)` pair in this map indicates that the the runtime
    /// `K` is blocked on some query executing in the runtime `V`.
    /// This encodes a graph that must be acyclic (or else deadlock
    /// will result).
    edges: FxHashMap<RuntimeId, RuntimeId>,
    labels: FxHashMap<DB::QueryDescriptor, SmallVec<[RuntimeId; 4]>>,
}

impl<DB: Database> Default for DependencyGraph<DB> {
    fn default() -> Self {
        DependencyGraph {
            edges: Default::default(),
            labels: Default::default(),
        }
    }
}

impl<DB: Database> DependencyGraph<DB> {
    /// Attempt to add an edge `from_id -> to_id` into the result graph.
    fn add_edge(
        &mut self,
        from_id: RuntimeId,
        descriptor: &DB::QueryDescriptor,
        to_id: RuntimeId,
    ) -> bool {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));

        // First: walk the chain of things that `to_id` depends on,
        // looking for us.
        let mut p = to_id;
        while let Some(&q) = self.edges.get(&p) {
            if q == from_id {
                return false;
            }

            p = q;
        }

        self.edges.insert(from_id, to_id);
        self.labels
            .entry(descriptor.clone())
            .or_insert(SmallVec::default())
            .push(from_id);
        true
    }

    fn remove_edge(&mut self, descriptor: &DB::QueryDescriptor, to_id: RuntimeId) {
        let vec = self
            .labels
            .remove(descriptor)
            .unwrap_or(SmallVec::default());

        for from_id in &vec {
            let to_id1 = self.edges.remove(from_id);
            assert_eq!(Some(to_id), to_id1);
        }
    }
}
