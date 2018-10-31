use crate::{Database, Event, EventKind, ParallelDatabase, SweepStrategy};
use lock_api::{RawRwLock, RawRwLockRecursive};
use log::debug;
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
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

    /// As with `Database::fork_mut`, creates a second handle to this
    /// runtime meant to be used from another thread.
    ///
    /// **Warning.** This second handle is intended to be used from a
    /// separate thread. Using two database handles from the **same
    /// thread** can lead to deadlock.
    pub fn fork_mut(&self) -> Self {
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
        self.with_incremented_revision(|_| ());
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

    /// The unique identifier attached to this `SalsaRuntime`. Each
    /// forked runtime has a distinct identifier.
    #[inline]
    pub fn id(&self) -> RuntimeId {
        self.id
    }

    /// Returns the descriptor for the query that this thread is
    /// actively executing (if any).
    pub fn active_query(&self) -> Option<DB::QueryDescriptor> {
        self.local_state
            .borrow()
            .query_stack
            .last()
            .map(|active_query| active_query.descriptor.clone())
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

    /// Acquires the **global query write lock** (ensuring that no
    /// queries are executing) and then increments the current
    /// revision counter; invokes `op` with the global query write
    /// lock still held.
    ///
    /// While we wait to acquire the global query write lock, this
    /// method will also increment `pending_revision_increments`, thus
    /// signalling to queries that their results are "canceled" and
    /// they should abort as expeditiously as possible.
    pub(crate) fn with_incremented_revision<R>(&self, op: impl FnOnce(Revision) -> R) -> R {
        log::debug!("increment_revision()");

        if self.query_in_progress() {
            panic!("increment_revision invoked during a query computation");
        }

        // Signal that we have a pending increment so that workers can
        // start to cancel work.
        let old_pending_revision_increments = self
            .shared_state
            .pending_revision_increments
            .fetch_add(1, Ordering::SeqCst);
        assert!(
            old_pending_revision_increments != usize::max_value(),
            "pending increment overflow"
        );

        // To modify the revision, we need the lock.
        let _lock = self.shared_state.query_lock.write();

        // *Before* updating the revision number, decrement the
        // `pending_revision_increments` counter. This way, if anybody
        // should happen to invoke `is_current_revision_canceled`
        // before we update the number, and they read 0, they don't
        // get an incorrect result -- once they acquire the query
        // lock, we'll be in the new revision.
        self.shared_state
            .pending_revision_increments
            .fetch_sub(1, Ordering::SeqCst);

        let old_revision = self.shared_state.revision.fetch_add(1, Ordering::SeqCst);
        assert!(old_revision != usize::max_value(), "revision overflow");

        let new_revision = Revision {
            generation: 1 + old_revision as u64,
        };
        debug!("increment_revision: incremented to {:?}", new_revision);

        op(new_revision)
    }

    pub(crate) fn query_in_progress(&self) -> bool {
        self.local_state.borrow().query_in_progress
    }

    pub(crate) fn execute_query_implementation<V>(
        &self,
        db: &DB,
        descriptor: &DB::QueryDescriptor,
        execute: impl FnOnce() -> V,
    ) -> ComputedQueryResult<DB, V> {
        debug!("{:?}: execute_query_implementation invoked", descriptor);

        db.salsa_event(|| Event {
            runtime_id: db.salsa_runtime().id(),
            kind: EventKind::WillExecute {
                descriptor: descriptor.clone(),
            },
        });

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
pub struct RuntimeId {
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

/// The `Frozen` struct indicates a database handle which is locked at
/// a particular revision: any attempt to set the value of an input
/// (e.g., from another database handle) will block until the `Frozen`
/// database is dropped.
///
/// # Deadlock warning
///
/// Since attempts to set inputs are blocked until the `Frozen<DB>` is
/// dropped, this implies that any attempt to set an input *using* the
/// `Frozen<DB>` will deadlock instantly (because it will block the
/// thread that owns the `Frozen<DB>` and thus not permit the
/// `Frozen<DB>` to be dropped). In the future, we plan to refactor
/// the API to make such "instant deadlocks" impossible.
pub struct Frozen<DB>
where
    DB: ParallelDatabase,
{
    shared_state: Arc<SharedState<DB>>,
    db: DB,
}

impl<DB> Frozen<DB>
where
    DB: ParallelDatabase,
{
    /// Creates and returns a frozen handle to `source_db`.
    pub(crate) fn new(source_db: &DB) -> Self {
        let source_runtime = source_db.salsa_runtime();

        // Subtle point: if the source database is already executing a
        // query, then we want to use a "recursive read" lock.  Using
        // an ordinary read lock [may deadlock], since it could wind
        // up blocking indefinitely if there is a pending write (thus
        // preventing our caller from releasing their read lock, which
        // in turn ensures that the pending write will never
        // complete).
        //
        // We could just use recursive reads *always*, but that could
        // lead to starvation in the case where you have various
        // threads invoking get and set willy nilly. Not sure how
        // important it is to ensure that case works -- it's not a
        // recommended pattern -- but it seems (for now at least) easy
        // enough to do so.
        //
        // [may deadlock]: https://docs.rs/lock_api/0.1.3/lock_api/struct.RwLock.html#method.read
        let use_recursive_lock = source_runtime.query_in_progress();

        // Fork off a new database for us to use.
        let our_db = source_db.fork_mut();
        let our_runtime = our_db.salsa_runtime();

        // Set the `query_in_progress` flag permanently true for our
        // database to prevent queries that execute against it from
        // acquiring the read lock.
        {
            let mut local_state = our_runtime.local_state.borrow_mut();
            if local_state.query_in_progress {
                panic!("cannot use `Frozen::new` with a query in progress")
            }
            local_state.query_in_progress = true;
        }

        // OK, this is paranoia. *Technically speaking*, we don't
        // control the DB type, so it may "yield up" different
        // runtimes at different points. So we will save the
        // shared-state that we are operating on to be sure it does
        // not. This way, we are sure that we are invoking the "unpaired"
        // lock and unlock operations on the same lock.
        //
        // (Note that -- if people are being wacky -- we might be
        // changing the `query_in_progress` flag on the wrong local
        // state. That I believe can only trigger *deadlock* so I'm
        // not as worried, but perhaps I should be.)
        let shared_state = our_runtime.shared_state.clone();

        // Acquire the read-lock without using RAII. This requires the
        // unsafe keyword because, if we were to unlock the lock this
        // way, we would screw up other people using the safe APIs.
        unsafe {
            if use_recursive_lock {
                shared_state.query_lock.raw().lock_shared_recursive();
            } else {
                shared_state.query_lock.raw().lock_shared();
            }
        }

        Frozen {
            shared_state,
            db: our_db,
        }
    }
}

impl<DB> std::ops::Deref for Frozen<DB>
where
    DB: ParallelDatabase,
{
    type Target = DB;

    fn deref(&self) -> &DB {
        &self.db
    }
}

impl<DB> Drop for Frozen<DB>
where
    DB: ParallelDatabase,
{
    fn drop(&mut self) {
        // Release our read-lock without using RAII. As documented in
        // `Frozen::new` above, this requires the unsafe keyword.
        unsafe {
            self.shared_state.query_lock.raw().unlock_shared();
        }
    }
}
