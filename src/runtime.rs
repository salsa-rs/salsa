use crate::dependency::DatabaseSlot;
use crate::dependency::Dependency;
use crate::durability::Durability;
use crate::plumbing::CycleDetected;
use crate::revision::{AtomicRevision, Revision};
use crate::{CycleError, Database, Event, EventKind, SweepStrategy};
use futures::{future::BoxFuture, prelude::*};
use log::debug;
use parking_lot::lock_api::{RawRwLock, RawRwLockRecursive};
use parking_lot::{Mutex, RwLock};
use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;
use std::hash::{BuildHasherDefault, Hash};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub(crate) type FxIndexSet<K> = indexmap::IndexSet<K, BuildHasherDefault<FxHasher>>;

mod local_state;
use local_state::LocalState;

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
    local_state: LocalState<DB>,

    /// Shared state that is accessible via all runtimes.
    shared_state: Arc<SharedState<DB>>,
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
    /// useful for profiling scenarios, but it also has interactions
    /// with garbage collection. In general, a synthetic write to
    /// durability level D will cause the system to fully trace all
    /// queries of durability level D and below. When running a GC, then:
    ///
    /// - Synthetic writes will cause more derived values to be
    ///   *retained*.  This is because derived values are only
    ///   retained if they are traced, and a synthetic write can cause
    ///   more things to be traced.
    /// - Synthetic writes can cause more interned values to be
    ///   *collected*. This is because interned values can only be
    ///   collected if they were not yet traced in the current
    ///   revision. Therefore, if you issue a synthetic write, execute
    ///   some query Q, and then start collecting interned values, you
    ///   will be able to recycle interned values not used in Q.
    ///
    /// In general, then, one can do a "full GC" that retains only
    /// those things that are used by some query Q by (a) doing a
    /// synthetic write at `Durability::HIGH`, (b) executing the query
    /// Q and then (c) doing a sweep.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    pub fn synthetic_write(&mut self, durability: Durability) {
        self.with_incremented_revision(|guard| {
            guard.mark_durability_as_changed(durability);
        });
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

    /// Returns the database-key for the query that this thread is
    /// actively executing (if any).
    pub fn active_query(&self) -> Option<DB::DatabaseKey> {
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
    fn pending_revision(&self) -> Revision {
        self.shared_state.pending_revision.load()
    }

    /// Check if the current revision is canceled. If this method ever
    /// returns true, the currently executing query is also marked as
    /// having an *untracked read* -- this means that, in the next
    /// revision, we will always recompute its value "as if" some
    /// input had changed. This means that, if your revision is
    /// canceled (which indicates that current query results will be
    /// ignored) your query is free to shortcircuit and return
    /// whatever it likes.
    ///
    /// This method is useful for implementing cancellation of queries.
    /// You can do it in one of two ways, via `Result`s or via unwinding.
    ///
    /// The `Result` approach looks like this:
    ///
    ///   * Some queries invoke `is_current_revision_canceled` and
    ///     return a special value, like `Err(Canceled)`, if it returns
    ///     `true`.
    ///   * Other queries propagate the special value using `?` operator.
    ///   * API around top-level queries checks if the result is `Ok` or
    ///     `Err(Canceled)`.
    ///
    /// The `panic` approach works in a similar way:
    ///
    ///   * Some queries invoke `is_current_revision_canceled` and
    ///     panic with a special value, like `Canceled`, if it returns
    ///     true.
    ///   * The implementation of `Database` trait overrides
    ///     `on_propagated_panic` to throw this special value as well.
    ///     This way, panic gets propagated naturally through dependant
    ///     queries, even across the threads.
    ///   * API around top-level queries converts a `panic` into `Result` by
    ///     catching the panic (using either `std::panic::catch_unwind` or
    ///     threads) and downcasting the payload to `Canceled` (re-raising
    ///     panic if downcast fails).
    ///
    /// Note that salsa is explicitly designed to be panic-safe, so cancellation
    /// via unwinding is 100% valid approach to cancellation.
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
    pub(crate) fn with_incremented_revision<R>(
        &mut self,
        op: impl FnOnce(&DatabaseWriteLockGuard<'_, DB>) -> R,
    ) -> R {
        log::debug!("increment_revision()");

        if !self.permits_increment() {
            panic!("increment_revision invoked during a query computation");
        }

        // Set the `pending_revision` field so that people
        // know current revision is canceled.
        let current_revision = self.shared_state.pending_revision.fetch_then_increment();

        // To modify the revision, we need the lock.
        let shared_state = self.shared_state.clone();
        let _lock = shared_state.query_lock.write();

        let old_revision = self.shared_state.revisions[0].fetch_then_increment();
        assert_eq!(current_revision, old_revision);

        let new_revision = current_revision.next();

        debug!("increment_revision: incremented to {:?}", new_revision);

        op(&DatabaseWriteLockGuard {
            runtime: self,
            new_revision,
        })
    }

    pub(crate) fn permits_increment(&self) -> bool {
        self.revision_guard.is_none() && !self.local_state.query_in_progress()
    }

    pub(crate) async fn execute_query_implementation<V>(
        db: &mut DB,
        database_key: &DB::DatabaseKey,
        execute: impl for<'a> FnOnce(&'a mut DB) -> crate::BoxFutureLocal<'a, V>,
    ) -> ComputedQueryResult<DB, V> {
        debug!("{:?}: execute_query_implementation invoked", database_key);

        db.salsa_event(|| Event {
            runtime_id: db.salsa_runtime().id(),
            kind: EventKind::WillExecute {
                database_key: database_key.clone(),
            },
        });

        // Push the active query onto the stack.
        let max_durability = Durability::MAX;
        let active_query = LocalState::push_query(db, database_key, max_durability);

        // Execute user's code, accumulating inputs etc.
        let value = execute(active_query.db).await;

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
    pub(crate) fn report_query_read<'hack>(
        &self,
        database_slot: Arc<dyn DatabaseSlot<DB> + 'hack>,
        durability: Durability,
        changed_at: Revision,
    ) {
        let dependency = Dependency::new(database_slot);
        self.local_state
            .report_query_read(dependency, durability, changed_at);
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
        self.local_state.report_synthetic_read(durability);
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
        self.local_state.report_anon_read(revision)
    }

    /// Obviously, this should be user configurable at some point.
    pub(crate) fn report_unexpected_cycle(
        &self,
        database_key: &DB::DatabaseKey,
        error: CycleDetected,
        changed_at: Revision,
    ) -> crate::CycleError<DB::DatabaseKey> {
        debug!("report_unexpected_cycle(database_key={:?})", database_key);

        let mut query_stack = self.local_state.borrow_query_stack_mut();

        if error.from == error.to {
            // All queries in the cycle is local
            let start_index = query_stack
                .iter()
                .rposition(|active_query| active_query.database_key == *database_key)
                .unwrap();
            let mut cycle = Vec::new();
            let cycle_participants = &mut query_stack[start_index..];
            for active_query in &mut *cycle_participants {
                cycle.push(active_query.database_key.clone());
            }

            assert!(!cycle.is_empty());

            for active_query in cycle_participants {
                active_query.cycle = cycle.clone();
            }

            crate::CycleError {
                cycle,
                changed_at,
                durability: Durability::MAX,
            }
        } else {
            // Part of the cycle is on another thread so we need to lock and inspect the shared
            // state
            let dependency_graph = self.shared_state.dependency_graph.lock();

            let mut cycle = Vec::new();
            {
                let cycle_iter = dependency_graph
                    .get_cycle_path(
                        database_key,
                        error.to,
                        query_stack.iter().map(|query| &query.database_key),
                    )
                    .chain(Some(database_key));

                for key in cycle_iter {
                    cycle.push(key.clone());
                }
            }

            assert!(!cycle.is_empty());

            for active_query in query_stack
                .iter_mut()
                .filter(|query| cycle.iter().any(|key| *key == query.database_key))
            {
                active_query.cycle = cycle.clone();
            }

            crate::CycleError {
                cycle,
                changed_at,
                durability: Durability::MAX,
            }
        }
    }

    pub(crate) fn mark_cycle_participants(&self, err: &CycleError<DB::DatabaseKey>) {
        for active_query in self
            .local_state
            .borrow_query_stack_mut()
            .iter_mut()
            .rev()
            .take_while(|active_query| err.cycle.iter().any(|e| *e == active_query.database_key))
        {
            active_query.cycle = err.cycle.clone();
        }
    }

    /// Try to make this runtime blocked on `other_id`. Returns true
    /// upon success or false if `other_id` is already blocked on us.
    pub(crate) fn try_block_on(&self, database_key: &DB::DatabaseKey, other_id: RuntimeId) -> bool {
        self.shared_state.dependency_graph.lock().add_edge(
            self.id(),
            database_key,
            other_id,
            self.local_state
                .borrow_query_stack()
                .iter()
                .map(|query| query.database_key.clone()),
        )
    }

    pub(crate) fn unblock_queries_blocked_on_self(&self, database_key: &DB::DatabaseKey) {
        self.shared_state
            .dependency_graph
            .lock()
            .remove_edge(database_key, self.id())
    }
}

/// Temporary guard that indicates that the database write-lock is
/// held. You can get one of these by invoking
/// `with_incremented_revision`. It gives access to the new revision
/// and a few other operations that only make sense to do while an
/// update is happening.
pub(crate) struct DatabaseWriteLockGuard<'db, DB>
where
    DB: Database,
{
    runtime: &'db mut Runtime<DB>,
    new_revision: Revision,
}

impl<DB> DatabaseWriteLockGuard<'_, DB>
where
    DB: Database,
{
    pub(crate) fn new_revision(&self) -> Revision {
        self.new_revision
    }

    /// Indicates that this update modified an input marked as
    /// "constant". This will force re-evaluation of anything that was
    /// dependent on constants (which otherwise might not get
    /// re-evaluated).
    pub(crate) fn mark_durability_as_changed(&self, d: Durability) {
        for rev in &self.runtime.shared_state.revisions[1..=d.index()] {
            rev.store(self.new_revision);
        }
    }
}

/// State that will be common to all threads (when we support multiple threads)
struct SharedState<DB: Database> {
    storage: DB::DatabaseStorage,

    /// Stores the next id to use for a snapshotted runtime (starts at 1).
    next_id: AtomicU64,

    /// Whenever derived queries are executing, they acquire this lock
    /// in read mode. Mutating inputs (and thus creating a new
    /// revision) requires a write lock (thus guaranteeing that no
    /// derived queries are in progress). Note that this is not needed
    /// to prevent **race conditions** -- the revision counter itself
    /// is stored in an `AtomicU64` so it can be cheaply read
    /// without acquiring the lock.  Rather, the `query_lock` is used
    /// to ensure a higher-level consistency property.
    query_lock: RwLock<()>,

    /// This is typically equal to `revision` -- set to `revision+1`
    /// when a new revision is pending (which implies that the current
    /// revision is canceled).
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
    dependency_graph: Mutex<DependencyGraph<DB::DatabaseKey>>,
}

impl<DB: Database> SharedState<DB> {
    fn with_durabilities(durabilities: usize) -> Self {
        SharedState {
            next_id: AtomicU64::new(1),
            storage: Default::default(),
            query_lock: Default::default(),
            revisions: (0..durabilities).map(|_| AtomicRevision::start()).collect(),
            pending_revision: AtomicRevision::start(),
            dependency_graph: Default::default(),
        }
    }
}

impl<DB> std::panic::RefUnwindSafe for SharedState<DB>
where
    DB: Database,
    DB::DatabaseStorage: std::panic::RefUnwindSafe,
{
}

impl<DB: Database> Default for SharedState<DB> {
    fn default() -> Self {
        Self::with_durabilities(Durability::LEN)
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
            .field("revisions", &self.revisions)
            .field("pending_revision", &self.pending_revision)
            .finish()
    }
}

struct ActiveQuery<DB: Database> {
    /// What query is executing
    database_key: DB::DatabaseKey,

    /// Minimum durability of inputs observed so far.
    durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    changed_at: Revision,

    /// Set of subqueries that were accessed thus far, or `None` if
    /// there was an untracked the read.
    dependencies: Option<FxIndexSet<Dependency<DB>>>,

    /// Stores the entire cycle, if one is found and this query is part of it.
    cycle: Vec<DB::DatabaseKey>,
}

pub(crate) struct ComputedQueryResult<DB: Database, V> {
    /// Final value produced
    pub(crate) value: V,

    /// Minimum durability of inputs observed so far.
    pub(crate) durability: Durability,

    /// Maximum revision of all inputs observed. If we observe an
    /// untracked read, this will be set to the most recent revision.
    pub(crate) changed_at: Revision,

    /// Complete set of subqueries that were accessed, or `None` if
    /// there was an untracked the read.
    pub(crate) dependencies: Option<FxIndexSet<Dependency<DB>>>,

    /// The cycle if one occured while computing this value
    pub(crate) cycle: Vec<DB::DatabaseKey>,
}

impl<DB: Database> ActiveQuery<DB> {
    fn new(database_key: DB::DatabaseKey, max_durability: Durability) -> Self {
        ActiveQuery {
            database_key,
            durability: max_durability,
            changed_at: Revision::start(),
            dependencies: Some(FxIndexSet::default()),
            cycle: Vec::new(),
        }
    }

    fn add_read(&mut self, dependency: Dependency<DB>, durability: Durability, revision: Revision) {
        if let Some(set) = &mut self.dependencies {
            set.insert(dependency);
        }

        self.durability = self.durability.min(durability);
        self.changed_at = self.changed_at.max(revision);
    }

    fn add_untracked_read(&mut self, changed_at: Revision) {
        self.dependencies = None;
        self.durability = Durability::LOW;
        self.changed_at = changed_at;
    }

    fn add_synthetic_read(&mut self, durability: Durability) {
        self.durability = self.durability.min(durability);
    }

    fn add_anon_read(&mut self, changed_at: Revision) {
        self.changed_at = self.changed_at.max(changed_at);
    }
}

/// A unique identifier for a particular runtime. Each time you create
/// a snapshot, a fresh `RuntimeId` is generated. Once a snapshot is
/// complete, its `RuntimeId` may potentially be re-used.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuntimeId {
    counter: u64,
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct StampedValue<V> {
    pub(crate) value: V,
    pub(crate) durability: Durability,
    pub(crate) changed_at: Revision,
}

#[derive(Debug)]
struct Edge<K> {
    id: RuntimeId,
    path: Vec<K>,
}

#[derive(Debug)]
struct DependencyGraph<K: Hash + Eq> {
    /// A `(K -> V)` pair in this map indicates that the the runtime
    /// `K` is blocked on some query executing in the runtime `V`.
    /// This encodes a graph that must be acyclic (or else deadlock
    /// will result).
    edges: FxHashMap<RuntimeId, Edge<K>>,
    labels: FxHashMap<K, SmallVec<[RuntimeId; 4]>>,
}

impl<K> Default for DependencyGraph<K>
where
    K: Hash + Eq,
{
    fn default() -> Self {
        DependencyGraph {
            edges: Default::default(),
            labels: Default::default(),
        }
    }
}

impl<K> DependencyGraph<K>
where
    K: Hash + Eq + Clone,
{
    /// Attempt to add an edge `from_id -> to_id` into the result graph.
    fn add_edge(
        &mut self,
        from_id: RuntimeId,
        database_key: &K,
        to_id: RuntimeId,
        path: impl IntoIterator<Item = K>,
    ) -> bool {
        assert_ne!(from_id, to_id);
        debug_assert!(!self.edges.contains_key(&from_id));

        // First: walk the chain of things that `to_id` depends on,
        // looking for us.
        let mut p = to_id;
        while let Some(q) = self.edges.get(&p).map(|edge| edge.id) {
            if q == from_id {
                return false;
            }

            p = q;
        }

        self.edges.insert(
            from_id,
            Edge {
                id: to_id,
                path: path.into_iter().chain(Some(database_key.clone())).collect(),
            },
        );
        self.labels
            .entry(database_key.clone())
            .or_default()
            .push(from_id);
        true
    }

    fn remove_edge(&mut self, database_key: &K, to_id: RuntimeId) {
        let vec = self.labels.remove(database_key).unwrap_or_default();

        for from_id in &vec {
            let to_id1 = self.edges.remove(from_id).map(|edge| edge.id);
            assert_eq!(Some(to_id), to_id1);
        }
    }

    fn get_cycle_path<'a>(
        &'a self,
        database_key: &'a K,
        to: RuntimeId,
        local_path: impl IntoIterator<Item = &'a K>,
    ) -> impl Iterator<Item = &'a K>
    where
        K: std::fmt::Debug,
    {
        let mut current = Some((to, std::slice::from_ref(database_key)));
        let mut last = None;
        let mut local_path = Some(local_path);
        std::iter::from_fn(move || match current.take() {
            Some((id, path)) => {
                let link_key = path.last().unwrap();

                current = self.edges.get(&id).map(|edge| {
                    let i = edge.path.iter().rposition(|p| p == link_key).unwrap();
                    (edge.id, &edge.path[i + 1..])
                });

                if current.is_none() {
                    last = local_path.take().map(|local_path| {
                        local_path
                            .into_iter()
                            .skip_while(move |p| *p != link_key)
                            .skip(1)
                    });
                }

                Some(path)
            }
            None => match &mut last {
                Some(iter) => iter.next().map(std::slice::from_ref),
                None => None,
            },
        })
        .flat_map(|x| x)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_graph_path1() {
        let mut graph = DependencyGraph::default();
        let a = RuntimeId { counter: 0 };
        let b = RuntimeId { counter: 1 };
        assert!(graph.add_edge(a, &2, b, vec![1]));
        // assert!(graph.add_edge(b, &1, a, vec![3, 2]));
        assert_eq!(
            graph
                .get_cycle_path(&1, a, &[3, 2][..])
                .cloned()
                .collect::<Vec<i32>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn dependency_graph_path2() {
        let mut graph = DependencyGraph::default();
        let a = RuntimeId { counter: 0 };
        let b = RuntimeId { counter: 1 };
        let c = RuntimeId { counter: 2 };
        assert!(graph.add_edge(a, &3, b, vec![1]));
        assert!(graph.add_edge(b, &4, c, vec![2, 3]));
        // assert!(graph.add_edge(c, &1, a, vec![5, 6, 4, 7]));
        assert_eq!(
            graph
                .get_cycle_path(&1, a, &[5, 6, 4, 7][..])
                .cloned()
                .collect::<Vec<i32>>(),
            vec![1, 3, 4, 7]
        );
    }
}
