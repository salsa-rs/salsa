use crate::{Database, Event, EventKind, SweepStrategy};
use lock_api::{RawRwLock, RawRwLockRecursive};
use log::debug;
use parking_lot::{Mutex, RwLock};
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
    /// Our unique runtime id.
    id: RuntimeId,

    /// If this is a "forked" runtime, then the `revision_guard` will
    /// be `Some`; this guard holds a read-lock on the global query
    /// lock.
    revision_guard: Option<RevisionGuard<DB>>,

    /// Local state that is specific to this runtime (thread).
    local_state: RefCell<LocalState<DB>>,

    /// Shared state that is accessible via all runtimes.
    shared_state: Arc<SharedState<DB>>,
}

impl<DB> std::panic::RefUnwindSafe for Runtime<DB>
where
    DB: Database,
    DB::DatabaseStorage: std::panic::RefUnwindSafe,
{
}

impl<DB> Default for Runtime<DB>
where
    DB: Database,
{
    fn default() -> Self {
        Runtime {
            id: RuntimeId { counter: 0 },
            revision_guard: None,
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
            .field("forked", &self.revision_guard.is_some())
            .field("shared_state", &self.shared_state)
            .finish()
    }
}

impl<DB> Runtime<DB>
where
    DB: Database,
{
    /// Create a new runtime; equivalent to `Self::default`. This is
    /// used when creating a new database.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the underlying storage, where the keys/values for all queries are kept.
    pub fn storage(&self) -> &DB::DatabaseStorage {
        &self.shared_state.storage
    }

    /// Returns a "forked" runtime, suitable for use in a forked
    /// database. "Forked" runtimes hold a read-lock on the global
    /// state, which means that any attempt to `set` an input will
    /// block until the forked runtime is dropped. See
    /// `ParallelDatabase::snapshot` for more information.
    ///
    /// **Warning.** This second handle is intended to be used from a
    /// separate thread. Using two database handles from the **same
    /// thread** can lead to deadlock.
    pub fn snapshot(&self, from_db: &DB) -> Self {
        assert!(
            Arc::ptr_eq(&self.shared_state, &from_db.salsa_runtime().shared_state),
            "invoked `snapshot` with a non-matching database"
        );

        if self.local_state.borrow().query_in_progress() {
            panic!("it is not legal to `snapshot` during a query (see salsa-rs/salsa#80)");
        }

        let revision_guard = RevisionGuard::new(&self.shared_state);

        let id = RuntimeId {
            counter: self.shared_state.next_id.fetch_add(1, Ordering::SeqCst),
        };

        Runtime {
            id,
            revision_guard: Some(revision_guard),
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

    /// The unique identifier attached to this `SalsaRuntime`. Each
    /// snapshotted runtime has a distinct identifier.
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

    /// Read current value of the revision counter.
    #[inline]
    fn pending_revision(&self) -> Revision {
        Revision {
            generation: self.shared_state.pending_revision.load(Ordering::SeqCst) as u64,
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
        let current_revision = self.current_revision();
        let pending_revision = self.pending_revision();
        debug!(
            "is_current_revision_canceled: current_revision={:?}, pending_revision={:?}",
            current_revision, pending_revision
        );
        if pending_revision > current_revision {
            self.report_untracked_read();
            true
        } else {
            // Subtle: If the current revision is not canceled, we
            // still report an **anonymous** read, which will bump up
            // the revision number to be at least the last
            // non-canceled revision. This is needed to ensure
            // deterministic reads and avoid salsa-rs/salsa#66. The
            // specific scenario we are trying to avoid is tested by
            // `no_back_dating_in_cancellation`; it works like
            // this. Imagine we have 3 queries, where Query3 invokes
            // Query2 which invokes Query1. Then:
            //
            // - In Revision R1:
            //   - Query1: Observes cancelation and returns sentinel S.
            //     - Recorded inputs: Untracked, because we observed cancelation.
            //   - Query2: Reads Query1 and propagates sentinel S.
            //     - Recorded inputs: Query1, changed-at=R1
            //   - Query3: Reads Query2 and propagates sentinel S. (Inputs = Query2, ChangedAt R1)
            //     - Recorded inputs: Query2, changed-at=R1
            // - In Revision R2:
            //   - Query1: Observes no cancelation. All of its inputs last changed in R0,
            //     so it returns a valid value with "changed at" of R0.
            //     - Recorded inputs: ..., changed-at=R0
            //   - Query2: Recomputes its value and returns correct result.
            //     - Recorded inputs: Query1, changed-at=R0 <-- key problem!
            //   - Query3: sees that Query2's result last changed in R0, so it thinks it
            //     can re-use its value from R1 (which is the sentinel value).
            //
            // The anonymous read here prevents that scenario: Query1
            // winds up with a changed-at setting of R2, which is the
            // "pending revision", and hence Query2 and Query3
            // are recomputed.
            assert_eq!(pending_revision, current_revision);
            self.report_anon_read(pending_revision);
            false
        }
    }

    /// Calls the callback if the current revision is cancelled.
    ///
    /// Observing cancellation may lead to inconsistencies in database storage,
    /// so the callback must panic.
    pub fn if_current_revision_is_canceled(&self, cb: fn() -> !) {
        if self.pending_revision() > self.current_revision() {
            cb()
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
    ///
    /// Note that, given our writer model, we can assume that only one
    /// thread is attempting to increment the global revision at a
    /// time.
    pub(crate) fn with_incremented_revision<R>(&self, op: impl FnOnce(Revision) -> R) -> R {
        log::debug!("increment_revision()");

        if !self.permits_increment() {
            panic!("increment_revision invoked during a query computation");
        }

        // Set the `pending_revision` field so that people
        // know current revision is canceled.
        let current_revision = self
            .shared_state
            .pending_revision
            .fetch_add(1, Ordering::SeqCst);
        assert!(current_revision != usize::max_value(), "revision overflow");

        // To modify the revision, we need the lock.
        let _lock = self.shared_state.query_lock.write();

        let old_revision = self.shared_state.revision.fetch_add(1, Ordering::SeqCst);
        assert_eq!(current_revision, old_revision);

        let new_revision = Revision {
            generation: (current_revision + 1) as u64,
        };

        debug!("increment_revision: incremented to {:?}", new_revision);

        op(new_revision)
    }

    pub(crate) fn permits_increment(&self) -> bool {
        self.revision_guard.is_none() && !self.local_state.borrow().query_in_progress()
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

    /// An "anonymous" read is a read that doesn't come from executing
    /// a query, but from some other internal operation. It just
    /// modifies the "changed at" to be at least the given revision.
    /// (It also does not disqualify a query from being considered
    /// constant, since it is used for queries that don't give back
    /// actual *data*.)
    ///
    /// This is used when queries check if they have been canceled.
    fn report_anon_read(&self, revision: Revision) {
        if let Some(top_query) = self.local_state.borrow_mut().query_stack.last_mut() {
            top_query.add_anon_read(revision);
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

    /// Stores the next id to use for a snapshotted runtime (starts at 1).
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

    /// This is typically equal to `revision` -- set to `revision+1`
    /// when a new revision is pending (which implies that the current
    /// revision is canceled).
    pending_revision: AtomicUsize,

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
            pending_revision: Default::default(),
            dependency_graph: Default::default(),
        }
    }
}

impl<DB> std::fmt::Debug for SharedState<DB>
where
    DB: Database,
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let query_lock = if self.query_lock.try_write().is_some() {
            "<unlocked>"
        } else if self.query_lock.try_read().is_some() {
            "<rlocked>"
        } else {
            "<wlocked>"
        };
        fmt.debug_struct("SharedState")
            .field("query_lock", &query_lock)
            .field("revision", &self.revision)
            .field("pending_revision", &self.pending_revision)
            .finish()
    }
}

/// State that will be specific to a single execution threads (when we
/// support multiple threads)
struct LocalState<DB: Database> {
    query_stack: Vec<ActiveQuery<DB>>,
}

impl<DB: Database> Default for LocalState<DB> {
    fn default() -> Self {
        LocalState {
            query_stack: Default::default(),
        }
    }
}

impl<DB: Database> LocalState<DB> {
    fn query_in_progress(&self) -> bool {
        !self.query_stack.is_empty()
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

    fn add_anon_read(&mut self, changed_at: Revision) {
        self.changed_at.revision = self.changed_at.revision.max(changed_at);
    }
}

/// A unique identifier for a particular runtime. Each time you create
/// a snapshot, a fresh `RuntimeId` is generated. Once a snapshot is
/// complete, its `RuntimeId` may potentially be re-used.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuntimeId {
    counter: usize,
}

/// A unique identifier for the current version of the database; each
/// time an input is changed, the revision number is incremented.
/// `Revision` is used internally to track which values may need to be
/// recomputed, but not something you should have to interact with
/// directly as a user of salsa.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision {
    generation: u64,
}

impl Revision {
    pub(crate) const ZERO: Self = Revision { generation: 0 };

    fn next(self) -> Revision {
        Revision {
            generation: self.generation + 1,
        }
    }

    fn as_usize(self) -> usize {
        assert!(self.generation < (std::usize::MAX as u64));
        self.generation as usize
    }
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

struct RevisionGuard<DB: Database> {
    shared_state: Arc<SharedState<DB>>,
}

impl<DB> RevisionGuard<DB>
where
    DB: Database,
{
    fn new(shared_state: &Arc<SharedState<DB>>) -> Self {
        // Subtle: we use a "recursive" lock here so that it is not an
        // error to acquire a read-lock when one is already held (this
        // happens when a query uses `snapshot` to spawn off parallel
        // workers, for example).
        //
        // This has the side-effect that we are responsible to ensure
        // that people contending for the write lock do not starve,
        // but this is what we achieve via the cancellation mechanism.
        //
        // (In particular, since we only ever have one "mutating
        // handle" to the database, the only contention for the global
        // query lock occurs when there are "futures" evaluating
        // queries in parallel, and those futures hold a read-lock
        // already, so the starvation problem is more about them bring
        // themselves to a close, versus preventing other people from
        // *starting* work).
        unsafe {
            shared_state.query_lock.raw().lock_shared_recursive();
        }

        Self {
            shared_state: shared_state.clone(),
        }
    }
}

impl<DB> Drop for RevisionGuard<DB>
where
    DB: Database,
{
    fn drop(&mut self) {
        // Release our read-lock without using RAII. As documented in
        // `Snapshot::new` above, this requires the unsafe keyword.
        unsafe {
            self.shared_state.query_lock.raw().unlock_shared();
        }
    }
}
