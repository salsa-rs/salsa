use crate::durability::Durability;
use crate::plumbing::{CycleDetected, CycleError, CycleParticipants, CycleRecoveryStrategy};
use crate::revision::{AtomicRevision, Revision};
use crate::{Cancelled, Database, DatabaseKeyIndex, Event, EventKind};
use log::debug;
use parking_lot::lock_api::{RawRwLock, RawRwLockRecursive};
use parking_lot::{Mutex, RwLock};
use rustc_hash::FxHasher;
use std::hash::{BuildHasherDefault, Hash};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub(crate) type FxIndexSet<K> = indexmap::IndexSet<K, BuildHasherDefault<FxHasher>>;
pub(crate) type FxIndexMap<K, V> = indexmap::IndexMap<K, V, BuildHasherDefault<FxHasher>>;

mod dependency_graph;
use dependency_graph::DependencyGraph;

mod local_state;
use local_state::LocalState;

/// The salsa runtime stores the storage for all queries as well as
/// tracking the query stack and dependencies between cycles.
///
/// Each new runtime you create (e.g., via `Runtime::new` or
/// `Runtime::default`) will have an independent set of query storage
/// associated with it. Normally, therefore, you only do this once, at
/// the start of your application.
pub struct Runtime {
    /// Our unique runtime id.
    id: RuntimeId,

    /// If this is a "forked" runtime, then the `revision_guard` will
    /// be `Some`; this guard holds a read-lock on the global query
    /// lock.
    revision_guard: Option<RevisionGuard>,

    /// Local state that is specific to this runtime (thread).
    local_state: LocalState,

    /// Shared state that is accessible via all runtimes.
    shared_state: Arc<SharedState>,
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum WaitResult {
    Completed,
    Panicked,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            id: RuntimeId { counter: 0 },
            revision_guard: None,
            shared_state: Default::default(),
            local_state: Default::default(),
        }
    }
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Runtime")
            .field("id", &self.id())
            .field("forked", &self.revision_guard.is_some())
            .field("shared_state", &self.shared_state)
            .finish()
    }
}

impl Runtime {
    /// Create a new runtime; equivalent to `Self::default`. This is
    /// used when creating a new database.
    pub fn new() -> Self {
        Self::default()
    }

    /// See [`crate::storage::Storage::snapshot`].
    pub(crate) fn snapshot(&self) -> Self {
        if self.local_state.query_in_progress() {
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

    /// A "synthetic write" causes the system to act *as though* some
    /// input of durability `durability` has changed. This is mostly
    /// useful for profiling scenarios.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    pub fn synthetic_write(&mut self, durability: Durability) {
        self.with_incremented_revision(&mut |_next_revision| Some(durability));
    }

    /// The unique identifier attached to this `SalsaRuntime`. Each
    /// snapshotted runtime has a distinct identifier.
    #[inline]
    pub fn id(&self) -> RuntimeId {
        self.id
    }

    /// Returns the database-key for the query that this thread is
    /// actively executing (if any).
    pub fn active_query(&self) -> Option<DatabaseKeyIndex> {
        self.local_state.active_query()
    }

    /// Read current value of the revision counter.
    #[inline]
    pub(crate) fn current_revision(&self) -> Revision {
        self.shared_state.revisions[0].load()
    }

    /// The revision in which values with durability `d` may have last
    /// changed.  For D0, this is just the current revision. But for
    /// higher levels of durability, this value may lag behind the
    /// current revision. If we encounter a value of durability Di,
    /// then, we can check this function to get a "bound" on when the
    /// value may have changed, which allows us to skip walking its
    /// dependencies.
    #[inline]
    pub(crate) fn last_changed_revision(&self, d: Durability) -> Revision {
        self.shared_state.revisions[d.index()].load()
    }

    /// Read current value of the revision counter.
    #[inline]
    pub(crate) fn pending_revision(&self) -> Revision {
        self.shared_state.pending_revision.load()
    }

    #[cold]
    pub(crate) fn unwind_cancelled(&self) {
        self.report_untracked_read();
        Cancelled::throw();
    }

    /// Acquires the **global query write lock** (ensuring that no queries are
    /// executing) and then increments the current revision counter; invokes
    /// `op` with the global query write lock still held.
    ///
    /// While we wait to acquire the global query write lock, this method will
    /// also increment `pending_revision_increments`, thus signalling to queries
    /// that their results are "cancelled" and they should abort as expeditiously
    /// as possible.
    ///
    /// The `op` closure should actually perform the writes needed. It is given
    /// the new revision as an argument, and its return value indicates whether
    /// any pre-existing value was modified:
    ///
    /// - returning `None` means that no pre-existing value was modified (this
    ///   could occur e.g. when setting some key on an input that was never set
    ///   before)
    /// - returning `Some(d)` indicates that a pre-existing value was modified
    ///   and it had the durability `d`. This will update the records for when
    ///   values with each durability were modified.
    ///
    /// Note that, given our writer model, we can assume that only one thread is
    /// attempting to increment the global revision at a time.
    pub(crate) fn with_incremented_revision(
        &mut self,
        op: &mut dyn FnMut(Revision) -> Option<Durability>,
    ) {
        log::debug!("increment_revision()");

        if !self.permits_increment() {
            panic!("increment_revision invoked during a query computation");
        }

        // Set the `pending_revision` field so that people
        // know current revision is cancelled.
        let current_revision = self.shared_state.pending_revision.fetch_then_increment();

        // To modify the revision, we need the lock.
        let shared_state = self.shared_state.clone();
        let _lock = shared_state.query_lock.write();

        let old_revision = self.shared_state.revisions[0].fetch_then_increment();
        assert_eq!(current_revision, old_revision);

        let new_revision = current_revision.next();

        debug!("increment_revision: incremented to {:?}", new_revision);

        if let Some(d) = op(new_revision) {
            for rev in &self.shared_state.revisions[1..=d.index()] {
                rev.store(new_revision);
            }
        }
    }

    pub(crate) fn permits_increment(&self) -> bool {
        self.revision_guard.is_none() && !self.local_state.query_in_progress()
    }

    pub(crate) fn execute_query_implementation<DB, V>(
        &self,
        db: &DB,
        database_key_index: DatabaseKeyIndex,
        execute: impl FnOnce() -> V,
    ) -> ComputedQueryResult<V>
    where
        DB: ?Sized + Database,
    {
        debug!(
            "{:?}: execute_query_implementation invoked",
            database_key_index
        );

        db.salsa_event(Event {
            runtime_id: self.id(),
            kind: EventKind::WillExecute {
                database_key: database_key_index,
            },
        });

        // Push the active query onto the stack.
        let max_durability = Durability::MAX;
        let active_query = self
            .local_state
            .push_query(database_key_index, max_durability);

        // Execute user's code, accumulating inputs etc.
        let value = execute();

        // Extract accumulated inputs.
        let ActiveQuery {
            dependencies,
            changed_at,
            durability,
            cycle,
            ..
        } = active_query.complete();

        ComputedQueryResult {
            value,
            durability,
            changed_at,
            dependencies,
            cycle,
        }
    }

    /// Reports that the currently active query read the result from
    /// another query.
    ///
    /// # Parameters
    ///
    /// - `database_key`: the query whose result was read
    /// - `changed_revision`: the last revision in which the result of that
    ///   query had changed
    pub(crate) fn report_query_read(
        &self,
        input: DatabaseKeyIndex,
        durability: Durability,
        changed_at: Revision,
    ) {
        self.local_state
            .report_query_read(input, durability, changed_at);
    }

    /// Reports that the query depends on some state unknown to salsa.
    ///
    /// Queries which report untracked reads will be re-executed in the next
    /// revision.
    pub fn report_untracked_read(&self) {
        self.local_state
            .report_untracked_read(self.current_revision());
    }

    /// Acts as though the current query had read an input with the given durability; this will force the current query's durability to be at most `durability`.
    ///
    /// This is mostly useful to control the durability level for [on-demand inputs](https://salsa-rs.github.io/salsa/common_patterns/on_demand_inputs.html).
    pub fn report_synthetic_read(&self, durability: Durability) {
        self.local_state
            .report_synthetic_read(durability, self.current_revision());
    }

    /// Obviously, this should be user configurable at some point.
    pub(crate) fn report_unexpected_cycle(
        &self,
        db: &dyn Database,
        database_key_index: DatabaseKeyIndex,
        error: CycleDetected,
        changed_at: Revision,
    ) -> (CycleRecoveryStrategy, crate::CycleError) {
        debug!(
            "report_unexpected_cycle(database_key={:?})",
            database_key_index
        );

        let mut cycle_participants = vec![];
        let mut stack = self.local_state.take_query_stack();
        let mut dg = self.shared_state.dependency_graph.lock();
        dg.for_each_cycle_participant(error.from, &mut stack, database_key_index, error.to, |aq| {
            cycle_participants.push(aq.database_key_index);
        });
        let cycle_participants = Arc::new(cycle_participants);
        dg.for_each_cycle_participant(error.from, &mut stack, database_key_index, error.to, |aq| {
            aq.cycle = Some(cycle_participants.clone());
        });
        self.local_state.restore_query_stack(stack);
        let crs = self.mutual_cycle_recovery_strategy(db, &cycle_participants);
        debug!(
            "cycle recovery strategy {:?} for participants {:?}",
            crs, cycle_participants
        );

        (
            crs,
            CycleError {
                cycle: cycle_participants,
                changed_at,
                durability: Durability::MAX,
            },
        )
    }

    fn mutual_cycle_recovery_strategy(
        &self,
        db: &dyn Database,
        cycle: &[DatabaseKeyIndex],
    ) -> CycleRecoveryStrategy {
        let crs = db.cycle_recovery_strategy(cycle[0]);
        if let Some(key) = cycle[1..]
            .iter()
            .copied()
            .find(|&key| db.cycle_recovery_strategy(key) != crs)
        {
            debug!("mutual_cycle_recovery_strategy: cycle had multiple strategies ({:?} for {:?} vs {:?} for {:?})",
                crs, cycle[0],
                db.cycle_recovery_strategy(key), key
            );
            CycleRecoveryStrategy::Panic
        } else {
            crs
        }
    }

    /// Try to make this runtime blocked on `other_id`. Returns true
    /// upon success or false if `other_id` is already blocked on us.
    pub(crate) fn try_block_on<QueryMutexGuard>(
        &self,
        db: &dyn Database,
        database_key: DatabaseKeyIndex,
        other_id: RuntimeId,
        query_mutex_guard: QueryMutexGuard,
    ) -> Result<WaitResult, CycleDetected> {
        let mut dg = self.shared_state.dependency_graph.lock();

        if self.id() == other_id || dg.depends_on(other_id, self.id()) {
            Err(CycleDetected {
                from: self.id(),
                to: other_id,
            })
        } else {
            db.salsa_event(Event {
                runtime_id: self.id(),
                kind: EventKind::WillBlockOn {
                    other_runtime_id: other_id,
                    database_key: database_key,
                },
            });

            let stack = self.local_state.take_query_stack();

            let (stack, result) = DependencyGraph::block_on(
                dg,
                self.id(),
                database_key,
                other_id,
                stack,
                query_mutex_guard,
            );

            self.local_state.restore_query_stack(stack);

            Ok(result)
        }
    }

    /// Invoked when this runtime completed computing `database_key` with
    /// the given result `wait_result` (`wait_result` should be `None` if
    /// computing `database_key` panicked and could not complete).
    /// This function unblocks any dependent queries and allows them
    /// to continue executing.
    pub(crate) fn unblock_queries_blocked_on(
        &self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        self.shared_state
            .dependency_graph
            .lock()
            .unblock_dependents_of(database_key, self.id(), wait_result);
    }
}

/// State that will be common to all threads (when we support multiple threads)
struct SharedState {
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

    /// This is typically equal to `revision` -- set to `revision+1`
    /// when a new revision is pending (which implies that the current
    /// revision is cancelled).
    pending_revision: AtomicRevision,

    /// Stores the "last change" revision for values of each duration.
    /// This vector is always of length at least 1 (for Durability 0)
    /// but its total length depends on the number of durations. The
    /// element at index 0 is special as it represents the "current
    /// revision".  In general, we have the invariant that revisions
    /// in here are *declining* -- that is, `revisions[i] >=
    /// revisions[i + 1]`, for all `i`. This is because when you
    /// modify a value with durability D, that implies that values
    /// with durability less than D may have changed too.
    revisions: Vec<AtomicRevision>,

    /// The dependency graph tracks which runtimes are blocked on one
    /// another, waiting for queries to terminate.
    dependency_graph: Mutex<DependencyGraph>,
}

impl SharedState {
    fn with_durabilities(durabilities: usize) -> Self {
        SharedState {
            next_id: AtomicUsize::new(1),
            query_lock: Default::default(),
            revisions: (0..durabilities).map(|_| AtomicRevision::start()).collect(),
            pending_revision: AtomicRevision::start(),
            dependency_graph: Default::default(),
        }
    }
}

impl std::panic::RefUnwindSafe for SharedState {}

impl Default for SharedState {
    fn default() -> Self {
        Self::with_durabilities(Durability::LEN)
    }
}

impl std::fmt::Debug for SharedState {
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
            .field("revisions", &self.revisions)
            .field("pending_revision", &self.pending_revision)
            .finish()
    }
}

#[derive(Debug)]
struct ActiveQuery {
    /// What query is executing
    database_key_index: DatabaseKeyIndex,

    /// Minimum durability of inputs observed so far.
    durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    changed_at: Revision,

    /// Set of subqueries that were accessed thus far, or `None` if
    /// there was an untracked the read.
    dependencies: Option<FxIndexSet<DatabaseKeyIndex>>,

    /// Stores the entire cycle, if one is found and this query is part of it.
    cycle: Option<CycleParticipants>,
}

pub(crate) struct ComputedQueryResult<V> {
    /// Final value produced
    pub(crate) value: V,

    /// Minimum durability of inputs observed so far.
    pub(crate) durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    pub(crate) changed_at: Revision,

    /// Complete set of subqueries that were accessed, or `None` if
    /// there was an untracked read.
    pub(crate) dependencies: Option<FxIndexSet<DatabaseKeyIndex>>,

    /// The cycle if one occured while computing this value
    pub(crate) cycle: Option<CycleParticipants>,
}

impl ActiveQuery {
    fn new(database_key_index: DatabaseKeyIndex, max_durability: Durability) -> Self {
        ActiveQuery {
            database_key_index,
            durability: max_durability,
            changed_at: Revision::start(),
            dependencies: Some(FxIndexSet::default()),
            cycle: None,
        }
    }

    fn add_read(&mut self, input: DatabaseKeyIndex, durability: Durability, revision: Revision) {
        if let Some(set) = &mut self.dependencies {
            set.insert(input);
        }

        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
    }

    fn add_untracked_read(&mut self, changed_at: Revision) {
        self.dependencies = None;
        self.durability = Durability::LOW;
        self.changed_at = changed_at;
    }

    fn add_synthetic_read(&mut self, durability: Durability, current_revision: Revision) {
        self.durability = self.durability.min(durability);
        self.changed_at = current_revision;
    }
}

/// A unique identifier for a particular runtime. Each time you create
/// a snapshot, a fresh `RuntimeId` is generated. Once a snapshot is
/// complete, its `RuntimeId` may potentially be re-used.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuntimeId {
    counter: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct StampedValue<V> {
    pub(crate) value: V,
    pub(crate) durability: Durability,
    pub(crate) changed_at: Revision,
}

struct RevisionGuard {
    shared_state: Arc<SharedState>,
}

impl RevisionGuard {
    fn new(shared_state: &Arc<SharedState>) -> Self {
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

impl Drop for RevisionGuard {
    fn drop(&mut self) {
        // Release our read-lock without using RAII. As documented in
        // `Snapshot::new` above, this requires the unsafe keyword.
        unsafe {
            self.shared_state.query_lock.raw().unlock_shared();
        }
    }
}
