use crate::debug::TableEntry;
use crate::plumbing::CycleDetected;
use crate::plumbing::DatabaseKey;
use crate::plumbing::LruQueryStorageOps;
use crate::plumbing::QueryFunction;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::runtime::ChangedAt;
use crate::runtime::FxIndexSet;
use crate::runtime::Revision;
use crate::runtime::Runtime;
use crate::runtime::RuntimeId;
use crate::runtime::StampedValue;
use crate::{Database, DiscardIf, DiscardWhat, Event, EventKind, SweepStrategy};
use linked_hash_map::LinkedHashMap;
use log::{debug, info};
use parking_lot::Mutex;
use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;
use std::hash::BuildHasherDefault;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

/// Memoized queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub type MemoizedStorage<DB, Q> = DerivedStorage<DB, Q, AlwaysMemoizeValue>;

/// "Dependency" queries just track their dependencies and not the
/// actual value (which they produce on demand). This lessens the
/// storage requirements.
pub type DependencyStorage<DB, Q> = DerivedStorage<DB, Q, NeverMemoizeValue>;

/// Handles storage where the value is 'derived' by executing a
/// function (in contrast to "inputs").
pub struct DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    // `lru_cap` logically belongs to `QueryMap`, but we store it outside, so
    // that we can read it without aquiring the lock.
    lru_cap: AtomicUsize,
    map: RwLock<QueryMap<DB, Q>>,
    policy: PhantomData<MP>,
}

impl<DB, Q, MP> std::panic::RefUnwindSafe for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: std::panic::RefUnwindSafe,
{
}

pub trait MemoizationPolicy<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(key: &Q::Key) -> bool;

    fn memoized_value_eq(old_value: &Q::Value, new_value: &Q::Value) -> bool;
}

pub enum AlwaysMemoizeValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for AlwaysMemoizeValue
where
    Q: QueryFunction<DB>,
    Q::Value: Eq,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        true
    }

    fn memoized_value_eq(old_value: &Q::Value, new_value: &Q::Value) -> bool {
        old_value == new_value
    }
}

pub enum NeverMemoizeValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for NeverMemoizeValue
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        false
    }

    fn memoized_value_eq(_old_value: &Q::Value, _new_value: &Q::Value) -> bool {
        panic!("cannot reach since we never memoize")
    }
}

/// Defines the "current state" of query's memoized results.
enum QueryState<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    /// The runtime with the given id is currently computing the
    /// result of this query; if we see this value in the table, it
    /// indeeds a cycle.
    InProgress {
        id: RuntimeId,
        waiting: Mutex<SmallVec<[Sender<StampedValue<Q::Value>>; 2]>>,
    },

    /// We have computed the query already, and here is the result.
    Memoized(Memo<DB, Q>),
}

impl<DB, Q> QueryState<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn in_progress(id: RuntimeId) -> Self {
        QueryState::InProgress {
            id,
            waiting: Default::default(),
        }
    }

    fn value(&self) -> Option<Q::Value> {
        match self {
            QueryState::InProgress { .. } => None,
            QueryState::Memoized(memo) => memo.value.clone(),
        }
    }
}

struct Memo<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    /// The result of the query, if we decide to memoize it.
    value: Option<Q::Value>,

    /// Last revision when this memo was verified (if there are
    /// untracked inputs, this will also be when the memo was
    /// created).
    verified_at: Revision,

    /// Last revision when the memoized value was observed to change.
    changed_at: Revision,

    /// The inputs that went into our query, if we are tracking them.
    inputs: MemoInputs<DB>,
}

/// An insertion-order-preserving set of queries. Used to track the
/// inputs accessed during query execution.
pub(crate) enum MemoInputs<DB: Database> {
    // No inputs
    Constant,

    // Non-empty set of inputs fully known
    Tracked {
        inputs: Arc<FxIndexSet<DB::DatabaseKey>>,
    },

    // Unknown quantity of inputs
    Untracked,
}

impl<DB: Database> MemoInputs<DB> {
    fn is_constant(&self) -> bool {
        if let MemoInputs::Constant = self {
            true
        } else {
            false
        }
    }
}

impl<DB: Database> std::fmt::Debug for MemoInputs<DB> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoInputs::Constant => fmt.debug_struct("Constant").finish(),
            MemoInputs::Tracked { inputs } => {
                fmt.debug_struct("Tracked").field("inputs", inputs).finish()
            }
            MemoInputs::Untracked => fmt.debug_struct("Untracked").finish(),
        }
    }
}

type LinkedHashSet<T> = LinkedHashMap<T, (), BuildHasherDefault<FxHasher>>;

struct QueryMap<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    lru_keys: LinkedHashSet<Q::Key>,
    data: FxHashMap<Q::Key, QueryState<DB, Q>>,
}

impl<DB, Q> Default for QueryMap<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn default() -> Self {
        QueryMap {
            lru_keys: Default::default(),
            data: Default::default(),
        }
    }
}

impl<DB, Q> QueryMap<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn set_lru_capacity(&mut self, new_capacity: usize) {
        if new_capacity == 0 {
            self.lru_keys.clear();
        } else {
            while self.lru_keys.len() > new_capacity {
                self.remove_lru();
            }
            let additional_cap = new_capacity - self.lru_keys.len();
            self.lru_keys.reserve(additional_cap);
        }
    }

    fn record_use(&mut self, key: &Q::Key, lru_cap: usize) {
        self.lru_keys.insert(key.clone(), ());
        if self.lru_keys.len() > lru_cap {
            self.remove_lru();
        }
    }

    fn remove_lru(&mut self) {
        if let Some((evicted, ())) = self.lru_keys.pop_front() {
            if let Some(QueryState::Memoized(memo)) = self.data.get_mut(&evicted) {
                // Similar to GC, evicting a value with an untracked input could
                // lead to inconsistencies. Note that we can't check
                // `has_untracked_input` when we add the value to the cache,
                // because inputs can become untracked in the next revision.
                if memo.has_untracked_input() {
                    return;
                }
                memo.value = None;
            }
        }
    }
}

impl<DB, Q, MP> Default for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn default() -> Self {
        DerivedStorage {
            lru_cap: AtomicUsize::new(0),
            map: RwLock::new(QueryMap::default()),
            policy: PhantomData,
        }
    }
}

/// Return value of `probe` helper.
enum ProbeState<V, G> {
    UpToDate(Result<V, CycleDetected>),
    StaleOrAbsent(G),
}

impl<DB, Q, MP> DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn read(
        &self,
        db: &DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        let runtime = db.salsa_runtime();

        // NB: We don't need to worry about people modifying the
        // revision out from under our feet. Either `db` is a frozen
        // database, in which case there is a lock, or the mutator
        // thread is the current thread, and it will be prevented from
        // doing any `set` invocations while the query function runs.
        let revision_now = runtime.current_revision();

        info!(
            "{:?}({:?}): invoked at {:?}",
            Q::default(),
            key,
            revision_now,
        );

        // First, do a check with a read-lock.
        match self.probe(
            db,
            self.map.read(),
            runtime,
            revision_now,
            database_key,
            key,
        ) {
            ProbeState::UpToDate(v) => return v,
            ProbeState::StaleOrAbsent(_guard) => (),
        }

        self.read_upgrade(db, key, database_key, revision_now)
    }

    /// Second phase of a read operation: acquires an upgradable-read
    /// and -- if needed -- validates whether inputs have changed,
    /// recomputes value, etc. This is invoked after our initial probe
    /// shows a potentially out of date value.
    fn read_upgrade(
        &self,
        db: &DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
        revision_now: Revision,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        let runtime = db.salsa_runtime();

        debug!(
            "{:?}({:?}): read_upgrade(revision_now={:?})",
            Q::default(),
            key,
            revision_now,
        );

        // Check with an upgradable read to see if there is a value
        // already. (This permits other readers but prevents anyone
        // else from running `read_upgrade` at the same time.)
        //
        // FIXME(Amanieu/parking_lot#101) -- we are using a write-lock
        // and not an upgradable read here because upgradable reads
        // can sometimes encounter deadlocks.
        let old_memo = match self.probe(
            db,
            self.map.write(),
            runtime,
            revision_now,
            database_key,
            key,
        ) {
            ProbeState::UpToDate(v) => return v,
            ProbeState::StaleOrAbsent(mut map) => {
                match map
                    .data
                    .insert(key.clone(), QueryState::in_progress(runtime.id()))
                {
                    Some(QueryState::Memoized(old_memo)) => Some(old_memo),
                    Some(QueryState::InProgress { .. }) => unreachable!(),
                    None => None,
                }
            }
        };

        let mut panic_guard = PanicGuard::new(&self.map, key, old_memo, database_key, runtime);

        // If we have an old-value, it *may* now be stale, since there
        // has been a new revision since the last time we checked. So,
        // first things first, let's walk over each of our previous
        // inputs and check whether they are out of date.
        if let Some(memo) = &mut panic_guard.memo {
            if let Some(value) = memo.validate_memoized_value(db, revision_now) {
                info!(
                    "{:?}({:?}): validated old memoized value",
                    Q::default(),
                    key
                );

                db.salsa_event(|| Event {
                    runtime_id: runtime.id(),
                    kind: EventKind::DidValidateMemoizedValue {
                        database_key: database_key.clone(),
                    },
                });

                panic_guard.proceed(&value);

                return Ok(value);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let mut result = runtime.execute_query_implementation(db, database_key, || {
            info!("{:?}({:?}): executing query", Q::default(), key);
            Q::execute(db, key.clone())
        });

        // We assume that query is side-effect free -- that is, does
        // not mutate the "inputs" to the query system. Sanity check
        // that assumption here, at least to the best of our ability.
        assert_eq!(
            runtime.current_revision(),
            revision_now,
            "revision altered during query execution",
        );

        // If the new value is equal to the old one, then it didn't
        // really change, even if some of its inputs have. So we can
        // "backdate" its `changed_at` revision to be the same as the
        // old value.
        if let Some(old_memo) = &panic_guard.memo {
            if let Some(old_value) = &old_memo.value {
                if MP::memoized_value_eq(&old_value, &result.value) {
                    debug!(
                        "read_upgrade({:?}({:?})): value is equal, back-dating to {:?}",
                        Q::default(),
                        key,
                        old_memo.changed_at,
                    );

                    assert!(old_memo.changed_at <= result.changed_at.revision);
                    result.changed_at.revision = old_memo.changed_at;
                }
            }
        }

        let new_value = StampedValue {
            value: result.value,
            changed_at: result.changed_at,
        };

        let value = if self.should_memoize_value(key) {
            Some(new_value.value.clone())
        } else {
            None
        };

        debug!(
            "read_upgrade({:?}({:?})): result.changed_at={:?}, result.subqueries = {:#?}",
            Q::default(),
            key,
            result.changed_at,
            result.subqueries,
        );

        let inputs = match result.subqueries {
            None => MemoInputs::Untracked,

            Some(database_keys) => {
                // If all things that we read were constants, then
                // we don't need to track our inputs: our value
                // can never be invalidated.
                //
                // If OTOH we read at least *some* non-constant
                // inputs, then we do track our inputs (even the
                // constants), so that if we run the GC, we know
                // which constants we looked at.
                if database_keys.is_empty() || result.changed_at.is_constant {
                    MemoInputs::Constant
                } else {
                    MemoInputs::Tracked {
                        inputs: Arc::new(database_keys),
                    }
                }
            }
        };
        panic_guard.memo = Some(Memo {
            value,
            changed_at: result.changed_at.revision,
            verified_at: revision_now,
            inputs,
        });

        panic_guard.proceed(&new_value);

        Ok(new_value)
    }

    /// Helper for `read`:
    ///
    /// Invoked with the guard `map` of some lock on `self.map` (read
    /// or write) as well as details about the key to look up.  Looks
    /// in the map to see if we have an up-to-date value or a
    /// cycle. Returns a suitable `ProbeState`:
    ///
    /// - `ProbeState::UpToDate(r)` if the table has an up-to-date
    ///   value (or we blocked on another thread that produced such a value).
    /// - `ProbeState::CycleDetected` if this thread is (directly or
    ///   indirectly) already computing this value.
    /// - `ProbeState::BlockedOnOtherThread` if some other thread
    ///   (which does not depend on us) was already computing this
    ///   value; caller should re-acquire the lock and try again.
    /// - `ProbeState::StaleOrAbsent` if either (a) there is no memo
    ///    for this key, (b) the memo has no value; or (c) the memo
    ///    has not been verified at the current revision.
    ///
    /// Note that in all cases **except** for `StaleOrAbsent`, the lock on
    /// `map` will have been released.
    fn probe<MapGuard>(
        &self,
        db: &DB,
        map: MapGuard,
        runtime: &Runtime<DB>,
        revision_now: Revision,
        database_key: &DB::DatabaseKey,
        key: &Q::Key,
    ) -> ProbeState<StampedValue<Q::Value>, MapGuard>
    where
        MapGuard: Deref<Target = QueryMap<DB, Q>>,
    {
        match map.data.get(key) {
            Some(QueryState::InProgress { id, waiting }) => {
                let other_id = *id;
                return match self.register_with_in_progress_thread(
                    runtime,
                    database_key,
                    other_id,
                    waiting,
                ) {
                    Ok(rx) => {
                        // Release our lock on `self.map`, so other thread
                        // can complete.
                        std::mem::drop(map);

                        db.salsa_event(|| Event {
                            runtime_id: db.salsa_runtime().id(),
                            kind: EventKind::WillBlockOn {
                                other_runtime_id: other_id,
                                database_key: database_key.clone(),
                            },
                        });

                        let value = rx.recv().unwrap_or_else(|_| db.on_propagated_panic());
                        ProbeState::UpToDate(Ok(value))
                    }

                    Err(CycleDetected) => ProbeState::UpToDate(Err(CycleDetected)),
                };
            }

            Some(QueryState::Memoized(memo)) => {
                debug!("{:?}({:?}): found memoized value", Q::default(), key);

                if let Some(value) = memo.probe_memoized_value(revision_now) {
                    info!(
                        "{:?}({:?}): returning memoized value changed at {:?}",
                        Q::default(),
                        key,
                        value.changed_at
                    );

                    return ProbeState::UpToDate(Ok(value));
                }
            }

            None => {}
        }

        ProbeState::StaleOrAbsent(map)
    }

    /// Helper:
    ///
    /// When we encounter an `InProgress` indicator, we need to either
    /// report a cycle or else register ourselves to be notified when
    /// that work completes. This helper does that; it returns a port
    /// where you can wait for the final value that wound up being
    /// computed (but first drop the lock on the map).
    fn register_with_in_progress_thread(
        &self,
        runtime: &Runtime<DB>,
        database_key: &DB::DatabaseKey,
        other_id: RuntimeId,
        waiting: &Mutex<SmallVec<[Sender<StampedValue<Q::Value>>; 2]>>,
    ) -> Result<Receiver<StampedValue<Q::Value>>, CycleDetected> {
        if other_id == runtime.id() {
            return Err(CycleDetected);
        } else {
            if !runtime.try_block_on(database_key, other_id) {
                return Err(CycleDetected);
            }

            let (tx, rx) = mpsc::channel();

            // The reader of this will have to acquire map
            // lock, we don't need any particular ordering.
            waiting.lock().push(tx);

            Ok(rx)
        }
    }

    fn should_memoize_value(&self, key: &Q::Key) -> bool {
        MP::should_memoize_value(key)
    }
}

struct PanicGuard<'db, DB, Q>
where
    DB: Database,
    Q: QueryFunction<DB>,
{
    database_key: &'db DB::DatabaseKey,
    key: &'db Q::Key,
    memo: Option<Memo<DB, Q>>,
    map: &'db RwLock<QueryMap<DB, Q>>,
    runtime: &'db Runtime<DB>,
}

impl<'db, DB, Q> PanicGuard<'db, DB, Q>
where
    DB: Database + 'db,
    Q: QueryFunction<DB>,
{
    fn new(
        map: &'db RwLock<QueryMap<DB, Q>>,
        key: &'db Q::Key,
        memo: Option<Memo<DB, Q>>,
        database_key: &'db DB::DatabaseKey,
        runtime: &'db Runtime<DB>,
    ) -> Self {
        Self {
            database_key,
            key,
            memo,
            map,
            runtime,
        }
    }

    /// Proceed with our panic guard by overwriting the placeholder for `key`.
    /// Once that completes, ensure that our deconstructor is not run once we
    /// are out of scope.
    fn proceed(mut self, new_value: &StampedValue<Q::Value>) {
        self.overwrite_placeholder(Some(new_value));
        std::mem::forget(self)
    }

    /// Overwrites the `InProgress` placeholder for `key` that we
    /// inserted; if others were blocked, waiting for us to finish,
    /// then notify them.
    fn overwrite_placeholder(&mut self, new_value: Option<&StampedValue<Q::Value>>) {
        let mut write = self.map.write();

        let old_value = match self.memo.take() {
            // Replace the `InProgress` marker that we installed with the new
            // memo, thus releasing our unique access to this key.
            Some(memo) => write
                .data
                .insert(self.key.clone(), QueryState::Memoized(memo)),

            // We had installed an `InProgress` marker, but we panicked before
            // it could be removed. At this point, we therefore "own" unique
            // access to our slot, so we can just remove the key.
            None => write.data.remove(self.key),
        };

        match old_value {
            Some(QueryState::InProgress { id, waiting }) => {
                assert_eq!(id, self.runtime.id());

                self.runtime
                    .unblock_queries_blocked_on_self(self.database_key);

                match new_value {
                    // If anybody has installed themselves in our "waiting"
                    // list, notify them that the value is available.
                    Some(new_value) => {
                        for tx in waiting.into_inner() {
                            tx.send(new_value.clone()).unwrap()
                        }
                    }

                    // We have no value to send when we are panicking.
                    // Therefore, we need to drop the sending half of the
                    // channel so that our panic propagates to those waiting
                    // on the receiving half.
                    None => std::mem::drop(waiting),
                }
            }
            _ => panic!(
                "\
Unexpected panic during query evaluation, aborting the process.

Please report this bug to https://github.com/salsa-rs/salsa/issues."
            ),
        }
    }
}

impl<'db, DB, Q> Drop for PanicGuard<'db, DB, Q>
where
    DB: Database + 'db,
    Q: QueryFunction<DB>,
{
    fn drop(&mut self) {
        if std::thread::panicking() {
            // We panicked before we could proceed and need to remove `key`.
            self.overwrite_placeholder(None)
        } else {
            // If no panic occurred, then panic guard ought to be
            // "forgotten" and so this Drop code should never run.
            panic!(".forget() was not called")
        }
    }
}

impl<DB, Q, MP> QueryStorageOps<DB, Q> for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn try_fetch(
        &self,
        db: &DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
    ) -> Result<Q::Value, CycleDetected> {
        let StampedValue { value, changed_at } = self.read(db, key, &database_key)?;

        let lru_cap = self.lru_cap.load(Ordering::Relaxed);
        if lru_cap > 0 {
            self.map.write().record_use(key, lru_cap);
        }

        db.salsa_runtime()
            .report_query_read(database_key, changed_at);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
    ) -> bool {
        let runtime = db.salsa_runtime();
        let revision_now = runtime.current_revision();

        debug!(
            "maybe_changed_since({:?}({:?})) called with revision={:?}, revision_now={:?}",
            Q::default(),
            key,
            revision,
            revision_now,
        );

        // Acquire read lock to start. In some of the arms below, we
        // drop this explicitly.
        let map = self.map.read();

        // Look for a memoized value.
        let memo = match map.data.get(key) {
            // If somebody depends on us, but we have no map
            // entry, that must mean that it was found to be out
            // of date and removed.
            None => {
                debug!(
                    "maybe_changed_since({:?}({:?}): no value",
                    Q::default(),
                    key,
                );
                return true;
            }

            // This value is being actively recomputed. Wait for
            // that thread to finish (assuming it's not dependent
            // on us...) and check its associated revision.
            Some(QueryState::InProgress { id, waiting }) => {
                let other_id = *id;
                debug!(
                    "maybe_changed_since({:?}({:?}): blocking on thread `{:?}`",
                    Q::default(),
                    key,
                    other_id,
                );
                match self.register_with_in_progress_thread(
                    runtime,
                    database_key,
                    other_id,
                    waiting,
                ) {
                    Ok(rx) => {
                        // Release our lock on `self.map`, so other thread
                        // can complete.
                        std::mem::drop(map);

                        let value = rx.recv().unwrap_or_else(|_| db.on_propagated_panic());
                        return value.changed_at.changed_since(revision);
                    }

                    // Consider a cycle to have changed.
                    Err(CycleDetected) => return true,
                }
            }

            Some(QueryState::Memoized(memo)) => memo,
        };

        if memo.verified_at == revision_now {
            debug!(
                "maybe_changed_since({:?}({:?}): {:?} since up-to-date memo that changed at {:?}",
                Q::default(),
                key,
                memo.changed_at > revision,
                memo.changed_at,
            );
            return memo.changed_at > revision;
        }

        let inputs = match &memo.inputs {
            MemoInputs::Untracked => {
                // we don't know the full set of
                // inputs, so if there is a new
                // revision, we must assume it is
                // dirty
                debug!(
                    "maybe_changed_since({:?}({:?}): true since untracked inputs",
                    Q::default(),
                    key,
                );
                return true;
            }

            MemoInputs::Constant => None,

            MemoInputs::Tracked { inputs } => {
                // At this point, the value may be dirty (we have
                // to check the database-keys). If we have a cached
                // value, we'll just fall back to invoking `read`,
                // which will do that checking (and a bit more) --
                // note that we skip the "pure read" part as we
                // already know the result.
                assert!(inputs.len() > 0);
                if memo.value.is_some() {
                    std::mem::drop(map);
                    return match self.read_upgrade(db, key, database_key, revision_now) {
                        Ok(v) => {
                            debug!(
                                "maybe_changed_since({:?}({:?}): {:?} since (recomputed) value changed at {:?}",
                                Q::default(),
                                key,
                                v.changed_at.changed_since(revision),
                                v.changed_at,
                            );
                            v.changed_at.changed_since(revision)
                        }
                        Err(CycleDetected) => true,
                    };
                }

                Some(inputs.clone())
            }
        };

        // We have a **tracked set of inputs**
        // (found in `database_keys`) that need to
        // be validated.
        std::mem::drop(map);

        // Iterate the inputs and see if any have maybe changed.
        let maybe_changed = inputs
            .iter()
            .flat_map(|inputs| inputs.iter())
            .filter(|input| input.maybe_changed_since(db, revision))
            .inspect(|input| {
                debug!(
                    "{:?}({:?}): input `{:?}` may have changed",
                    Q::default(),
                    key,
                    input
                )
            })
            .next()
            .is_some();

        // Either way, we have to update our entry.
        //
        // Keep in mind, though, we only acquired a read lock so a lot
        // could have happened in the interim. =) Therefore, we have
        // to probe the current state of `key` and in some cases we
        // ought to do nothing.
        {
            let mut map = self.map.write();
            match map.data.get_mut(key) {
                Some(QueryState::Memoized(memo)) => {
                    if memo.verified_at == revision_now {
                        // Since we started verifying inputs, somebody
                        // else has come along and updated this value
                        // (they may even have recomputed
                        // it). Therefore, we should not touch this
                        // memo.
                        //
                        // FIXME: Should we still return whatever
                        // `maybe_changed` value we computed,
                        // however..? It seems .. harmless to indicate
                        // that the value has changed, but possibly
                        // less efficient? (It may cause some
                        // downstream value to be recomputed that
                        // wouldn't otherwise have to be?)
                    } else if maybe_changed {
                        // We found this entry is out of date and
                        // nobody touch it in the meantime. Just
                        // remove it.
                        map.data.remove(key);
                    } else {
                        // We found this entry is valid. Update the
                        // `verified_at` to reflect the current
                        // revision.
                        memo.verified_at = revision_now;
                    }
                }

                Some(QueryState::InProgress { .. }) => {
                    // Since we started verifying inputs, somebody
                    // else has come along and started updated this
                    // value. Just leave their marker alone and return
                    // whatever `maybe_changed` value we computed.
                }

                None => {
                    // Since we started verifying inputs, somebody
                    // else has come along and removed this value. The
                    // GC can do this, for example. That's fine.
                }
            }
        }

        maybe_changed
    }

    fn is_constant(&self, _db: &DB, key: &Q::Key) -> bool {
        let map_read = self.map.read();
        match map_read.data.get(key) {
            None => false,
            Some(QueryState::InProgress { .. }) => panic!("query in progress"),
            Some(QueryState::Memoized(memo)) => memo.inputs.is_constant(),
        }
    }

    fn entries<C>(&self, _db: &DB) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        let map = self.map.read();
        map.data
            .iter()
            .map(|(key, query_state)| TableEntry::new(key.clone(), query_state.value()))
            .collect()
    }
}

impl<DB, Q, MP> QueryStorageMassOps<DB> for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn sweep(&self, db: &DB, strategy: SweepStrategy) {
        let mut map_write = self.map.write();
        let revision_now = db.salsa_runtime().current_revision();
        map_write.data.retain(|key, query_state| {
            match query_state {
                // Leave stuff that is currently being computed -- the
                // other thread doing that work has unique access to
                // this slot and we should not interfere.
                QueryState::InProgress { .. } => {
                    debug!("sweep({:?}({:?})): in-progress", Q::default(), key);
                    true
                }

                // Otherwise, drop only value or the whole memo accoring to the
                // strategy.
                QueryState::Memoized(memo) => {
                    debug!(
                        "sweep({:?}({:?})): last verified at {:?}, current revision {:?}",
                        Q::default(),
                        key,
                        memo.verified_at,
                        revision_now
                    );

                    // Check if this memo read something "untracked"
                    // -- meaning non-deterministic.  In this case, we
                    // can only collect "outdated" data that wasn't
                    // used in the current revision. This is because
                    // if we collected something from the current
                    // revision, we might wind up re-executing the
                    // query later in the revision and getting a
                    // distinct result.
                    let has_untracked_input = memo.has_untracked_input();

                    // Since we don't acquire a query lock in this
                    // method, it *is* possible for the revision to
                    // change while we are executing. However, it is
                    // *not* possible for any memos to have been
                    // written into this table that reflect the new
                    // revision, since we are holding the write lock
                    // when we read `revision_now`.
                    assert!(memo.verified_at <= revision_now);
                    match strategy.discard_if {
                        DiscardIf::Never => unreachable!(),

                        // If we are only discarding outdated things,
                        // and this is not outdated, keep it.
                        DiscardIf::Outdated if memo.verified_at == revision_now => true,

                        // As explained on the `has_untracked_input` variable
                        // definition, if this is a volatile entry, we
                        // can't discard it unless it is outdated.
                        DiscardIf::Always
                            if has_untracked_input && memo.verified_at == revision_now =>
                        {
                            true
                        }

                        // Otherwise, we can discard -- discard whatever the user requested.
                        DiscardIf::Outdated | DiscardIf::Always => match strategy.discard_what {
                            DiscardWhat::Nothing => unreachable!(),
                            DiscardWhat::Values => {
                                memo.value = None;
                                true
                            }
                            DiscardWhat::Everything => false,
                        },
                    }
                }
            }
        });
    }
}

impl<DB, Q, MP> LruQueryStorageOps for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn set_lru_capacity(&self, new_capacity: usize) {
        self.lru_cap.store(new_capacity, Ordering::SeqCst);
        self.map.write().set_lru_capacity(new_capacity);
    }
}

impl<DB, Q> Memo<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn validate_memoized_value(
        &mut self,
        db: &DB,
        revision_now: Revision,
    ) -> Option<StampedValue<Q::Value>> {
        // If we don't have a memoized value, nothing to validate.
        let value = self.value.as_ref()?;

        assert!(self.verified_at != revision_now);
        let verified_at = self.verified_at;

        debug!(
            "validate_memoized_value({:?}): verified_at={:#?}",
            Q::default(),
            self.inputs,
        );

        let is_constant = match &mut self.inputs {
            // We can't validate values that had untracked inputs; just have to
            // re-execute.
            MemoInputs::Untracked { .. } => {
                return None;
            }

            // Constant: no changed input
            MemoInputs::Constant => true,

            // Check whether any of our inputs changed since the
            // **last point where we were verified** (not since we
            // last changed). This is important: if we have
            // memoized values, then an input may have changed in
            // revision R2, but we found that *our* value was the
            // same regardless, so our change date is still
            // R1. But our *verification* date will be R2, and we
            // are only interested in finding out whether the
            // input changed *again*.
            MemoInputs::Tracked { inputs } => {
                let changed_input = inputs
                    .iter()
                    .filter(|input| input.maybe_changed_since(db, verified_at))
                    .next();

                if let Some(input) = changed_input {
                    debug!(
                        "{:?}::validate_memoized_value: `{:?}` may have changed",
                        Q::default(),
                        input
                    );

                    return None;
                }

                false
            }
        };

        self.verified_at = revision_now;
        Some(StampedValue {
            changed_at: ChangedAt {
                is_constant,
                revision: self.changed_at,
            },
            value: value.clone(),
        })
    }

    /// Returns the memoized value *if* it is known to be update in the given revision.
    fn probe_memoized_value(&self, revision_now: Revision) -> Option<StampedValue<Q::Value>> {
        let value = self.value.as_ref()?;

        debug!(
            "probe_memoized_value(verified_at={:?}, changed_at={:?})",
            self.verified_at, self.changed_at,
        );

        if self.verified_at == revision_now {
            let is_constant = match self.inputs {
                MemoInputs::Constant => true,
                _ => false,
            };

            return Some(StampedValue {
                changed_at: ChangedAt {
                    is_constant,
                    revision: self.changed_at,
                },
                value: value.clone(),
            });
        }

        None
    }

    fn has_untracked_input(&self) -> bool {
        match self.inputs {
            MemoInputs::Untracked => true,
            _ => false,
        }
    }
}
