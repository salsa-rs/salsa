use crate::plumbing::CycleDetected;
use crate::plumbing::QueryDescriptor;
use crate::plumbing::QueryFunction;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::plumbing::UncheckedMutQueryStorageOps;
use crate::runtime::ChangedAt;
use crate::runtime::FxIndexSet;
use crate::runtime::Revision;
use crate::runtime::Runtime;
use crate::runtime::RuntimeId;
use crate::runtime::StampedValue;
use crate::Database;
use crate::SweepStrategy;
use log::{debug, info};
use parking_lot::Mutex;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::marker::PhantomData;
use std::ops::Deref;
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

/// "Dependency" queries just track their dependencies and not the
/// actual value (which they produce on demand). This lessens the
/// storage requirements.
pub type VolatileStorage<DB, Q> = DerivedStorage<DB, Q, VolatileValue>;

/// Handles storage where the value is 'derived' by executing a
/// function (in contrast to "inputs").
pub struct DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    map: RwLock<FxHashMap<Q::Key, QueryState<DB, Q>>>,
    policy: PhantomData<MP>,
}

pub trait MemoizationPolicy<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(key: &Q::Key) -> bool;

    fn memoized_value_eq(old_value: &Q::Value, new_value: &Q::Value) -> bool;

    fn should_track_inputs(key: &Q::Key) -> bool;
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

    fn should_track_inputs(_key: &Q::Key) -> bool {
        true
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

    fn should_track_inputs(_key: &Q::Key) -> bool {
        true
    }
}

pub enum VolatileValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for VolatileValue
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        // Why memoize? Well, if the "volatile" value really is
        // constantly changing, we still want to capture its value
        // until the next revision is triggered and ensure it doesn't
        // change -- otherwise the system gets into an inconsistent
        // state where the same query reports back different values.
        true
    }

    fn memoized_value_eq(_old_value: &Q::Value, _new_value: &Q::Value) -> bool {
        false
    }

    fn should_track_inputs(_key: &Q::Key) -> bool {
        false
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
        inputs: Arc<FxIndexSet<DB::QueryDescriptor>>,
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

impl<DB, Q, MP> Default for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn default() -> Self {
        DerivedStorage {
            map: RwLock::new(FxHashMap::default()),
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
        descriptor: &DB::QueryDescriptor,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        let runtime = db.salsa_runtime();

        let _read_lock = runtime.start_query();

        let revision_now = runtime.current_revision();

        info!(
            "{:?}({:?}): invoked at {:?}",
            Q::default(),
            key,
            revision_now,
        );

        // First, do a check with a read-lock.
        match self.probe(self.map.read(), runtime, revision_now, descriptor, key) {
            ProbeState::UpToDate(v) => return v,
            ProbeState::StaleOrAbsent(_guard) => (),
        }

        self.read_upgrade(db, key, descriptor, revision_now)
    }

    /// Second phase of a read operation: acquires an upgradable-read
    /// and -- if needed -- validates whether inputs have changed,
    /// recomputes value, etc. This is invoked after our initial probe
    /// shows a potentially out of date value.
    fn read_upgrade(
        &self,
        db: &DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
        revision_now: Revision,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        let runtime = db.salsa_runtime();

        // Check with an upgradable read to see if there is a value
        // already. (This permits other readers but prevents anyone
        // else from running `read_upgrade` at the same time.)
        let mut old_memo = match self.probe(
            self.map.upgradable_read(),
            runtime,
            revision_now,
            descriptor,
            key,
        ) {
            ProbeState::UpToDate(v) => return v,
            ProbeState::StaleOrAbsent(map) => {
                let mut map = RwLockUpgradableReadGuard::upgrade(map);
                match map.insert(key.clone(), QueryState::in_progress(runtime.id())) {
                    Some(QueryState::Memoized(old_memo)) => Some(old_memo),
                    Some(QueryState::InProgress { .. }) => unreachable!(),
                    None => None,
                }
            }
        };

        let panic_guard = PanicGuard::new(&self.map, key, runtime.id());

        // If we have an old-value, it *may* now be stale, since there
        // has been a new revision since the last time we checked. So,
        // first things first, let's walk over each of our previous
        // inputs and check whether they are out of date.
        if let Some(memo) = &mut old_memo {
            if let Some(value) = memo.validate_memoized_value(db, revision_now) {
                info!(
                    "{:?}({:?}): validated old memoized value",
                    Q::default(),
                    key
                );

                self.overwrite_placeholder(
                    runtime,
                    descriptor,
                    key,
                    old_memo.unwrap(),
                    &value,
                    panic_guard,
                );
                return Ok(value);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let mut result = runtime.execute_query_implementation(descriptor, || {
            info!("{:?}({:?}): executing query", Q::default(), key);

            if !self.should_track_inputs(key) {
                runtime.report_untracked_read();
            }

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
        if let Some(old_memo) = &old_memo {
            if let Some(old_value) = &old_memo.value {
                if MP::memoized_value_eq(&old_value, &result.value) {
                    assert!(old_memo.changed_at <= result.changed_at.revision);
                    result.changed_at.revision = old_memo.changed_at;
                }
            }
        }

        let new_value = StampedValue {
            value: result.value,
            changed_at: result.changed_at,
        };

        {
            let value = if self.should_memoize_value(key) {
                Some(new_value.value.clone())
            } else {
                None
            };

            let inputs = match result.subqueries {
                None => MemoInputs::Untracked,

                Some(descriptors) => {
                    // If all things that we read were constants, then
                    // we don't need to track our inputs: our value
                    // can never be invalidated.
                    //
                    // If OTOH we read at least *some* non-constant
                    // inputs, then we do track our inputs (even the
                    // constants), so that if we run the GC, we know
                    // which constants we looked at.
                    if descriptors.is_empty() || result.changed_at.is_constant {
                        MemoInputs::Constant
                    } else {
                        MemoInputs::Tracked {
                            inputs: Arc::new(descriptors),
                        }
                    }
                }
            };

            self.overwrite_placeholder(
                runtime,
                descriptor,
                key,
                Memo {
                    value,
                    changed_at: result.changed_at.revision,
                    verified_at: revision_now,
                    inputs,
                },
                &new_value,
                panic_guard,
            );
        }

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
        map: MapGuard,
        runtime: &Runtime<DB>,
        revision_now: Revision,
        descriptor: &DB::QueryDescriptor,
        key: &Q::Key,
    ) -> ProbeState<StampedValue<Q::Value>, MapGuard>
    where
        MapGuard: Deref<Target = FxHashMap<Q::Key, QueryState<DB, Q>>>,
    {
        match map.get(key) {
            Some(QueryState::InProgress { id, waiting }) => {
                let other_id = *id;
                return match self
                    .register_with_in_progress_thread(runtime, descriptor, other_id, waiting)
                {
                    Ok(rx) => {
                        // Release our lock on `self.map`, so other thread
                        // can complete.
                        std::mem::drop(map);

                        let value = rx.recv().unwrap();
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
        descriptor: &DB::QueryDescriptor,
        other_id: RuntimeId,
        waiting: &Mutex<SmallVec<[Sender<StampedValue<Q::Value>>; 2]>>,
    ) -> Result<Receiver<StampedValue<Q::Value>>, CycleDetected> {
        if other_id == runtime.id() {
            return Err(CycleDetected);
        } else {
            if !runtime.try_block_on(descriptor, other_id) {
                return Err(CycleDetected);
            }

            let (tx, rx) = mpsc::channel();

            // The reader of this will have to acquire map
            // lock, we don't need any particular ordering.
            waiting.lock().push(tx);

            Ok(rx)
        }
    }

    /// Overwrites the `InProgress` placeholder for `key` that we
    /// inserted; if others were blocked, waiting for us to finish,
    /// the notify them.
    fn overwrite_placeholder(
        &self,
        runtime: &Runtime<DB>,
        descriptor: &DB::QueryDescriptor,
        key: &Q::Key,
        memo: Memo<DB, Q>,
        new_value: &StampedValue<Q::Value>,
        panic_guard: PanicGuard<'_, DB, Q>,
    ) {
        // No panic occurred, do not run the panic-guard destructor:
        panic_guard.forget();

        // Replace the in-progress placeholder that we installed with
        // the new memo, thus releasing our unique access to this
        // key. If anybody has installed themselves in our "waiting"
        // list, notify them that the value is available.
        let waiting = {
            let mut write = self.map.write();
            match write.insert(key.clone(), QueryState::Memoized(memo)) {
                Some(QueryState::InProgress { id, waiting }) => {
                    assert_eq!(id, runtime.id());

                    let waiting = waiting.into_inner();

                    if waiting.is_empty() {
                        // if nobody is waiting, we are done here
                        return;
                    }

                    runtime.unblock_queries_blocked_on_self(descriptor);

                    waiting
                }

                _ => panic!("expected in-progress state"),
            }
        };

        for tx in waiting {
            tx.send(new_value.clone()).unwrap();
        }
    }

    fn should_memoize_value(&self, key: &Q::Key) -> bool {
        MP::should_memoize_value(key)
    }

    fn should_track_inputs(&self, key: &Q::Key) -> bool {
        MP::should_track_inputs(key)
    }
}

struct PanicGuard<'db, DB, Q>
where
    DB: Database + 'db,
    Q: QueryFunction<DB>,
{
    my_id: RuntimeId,
    map: &'db RwLock<FxHashMap<Q::Key, QueryState<DB, Q>>>,
    key: &'db Q::Key,
}

impl<'db, DB, Q> PanicGuard<'db, DB, Q>
where
    DB: Database + 'db,
    Q: QueryFunction<DB>,
{
    fn new(
        map: &'db RwLock<FxHashMap<Q::Key, QueryState<DB, Q>>>,
        key: &'db Q::Key,
        my_id: RuntimeId,
    ) -> Self {
        Self { map, key, my_id }
    }

    fn forget(self) {
        std::mem::forget(self)
    }
}

impl<'db, DB, Q> Drop for PanicGuard<'db, DB, Q>
where
    DB: Database + 'db,
    Q: QueryFunction<DB>,
{
    fn drop(&mut self) {
        if std::thread::panicking() {
            // In this case, we had installed a `InProgress` marker but we
            // panicked before it could be removed. At this point, we
            // therefore "own" unique access to our slot, so we can just
            // remove the `InProgress` marker.

            let mut map = self.map.write();
            let old_value = map.remove(self.key);
            match old_value {
                Some(QueryState::InProgress { id, waiting }) => {
                    assert_eq!(id, self.my_id);

                    let waiting = waiting.into_inner();

                    if !waiting.is_empty() {
                        // FIXME(#24) -- handle parallel case. In
                        // particular, we ought to notify those
                        // waiting on us that a panic occurred (they
                        // can then propagate the panic themselves; or
                        // perhaps re-execute?).
                        panic!("FIXME(#24) -- handle parallel case");
                    }
                }

                // If we don't see an `InProgress` marker, something
                // has gone horribly wrong. This panic will
                // (unfortunately) abort the process, but recovery is
                // not possible.
                _ => panic!(
                    "\
Unexpected panic during query evaluation, aborting the process.

Please report this bug to https://github.com/salsa-rs/salsa/issues."
                ),
            }
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
        descriptor: &DB::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        let StampedValue { value, changed_at } = self.read(db, key, &descriptor)?;

        db.salsa_runtime().report_query_read(descriptor, changed_at);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> bool {
        let runtime = db.salsa_runtime();
        let revision_now = runtime.current_revision();

        // If a query is in progress, we know that the current
        // revision is not changing.
        if !runtime.query_in_progress() {
            panic!("maybe_changed_since invoked outside of query execution")
        }

        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?}, revision_now={:?})",
            Q::default(),
            key,
            revision,
            revision_now,
        );

        // Acquire read lock to start. In some of the arms below, we
        // drop this explicitly.
        let map = self.map.read();

        // Look for a memoized value.
        let memo = match map.get(key) {
            // If somebody depends on us, but we have no map
            // entry, that must mean that it was found to be out
            // of date and removed.
            None => return true,

            // This value is being actively recomputed. Wait for
            // that thread to finish (assuming it's not dependent
            // on us...) and check its associated revision.
            Some(QueryState::InProgress { id, waiting }) => {
                let other_id = *id;
                match self.register_with_in_progress_thread(runtime, descriptor, other_id, waiting)
                {
                    Ok(rx) => {
                        // Release our lock on `self.map`, so other thread
                        // can complete.
                        std::mem::drop(map);

                        let value = rx.recv().unwrap();
                        return value.changed_at.changed_since(revision);
                    }

                    // Consider a cycle to have changed.
                    Err(CycleDetected) => return true,
                }
            }

            Some(QueryState::Memoized(memo)) => memo,
        };

        if memo.verified_at == revision_now {
            return memo.changed_at > revision;
        }

        let inputs = match &memo.inputs {
            MemoInputs::Untracked => {
                // we don't know the full set of
                // inputs, so if there is a new
                // revision, we must assume it is
                // dirty
                return true;
            }

            MemoInputs::Constant => None,

            MemoInputs::Tracked { inputs } => {
                // At this point, the value may be dirty (we have
                // to check the descriptors). If we have a cached
                // value, we'll just fall back to invoking `read`,
                // which will do that checking (and a bit more) --
                // note that we skip the "pure read" part as we
                // already know the result.
                assert!(inputs.len() > 0);
                if memo.value.is_some() {
                    std::mem::drop(map);
                    return match self.read_upgrade(db, key, descriptor, revision_now) {
                        Ok(v) => v.changed_at.changed_since(revision),
                        Err(CycleDetected) => true,
                    };
                }

                Some(inputs.clone())
            }
        };

        // We have a **tracked set of inputs**
        // (found in `descriptors`) that need to
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
            match map.get_mut(key) {
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
                        map.remove(key);
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
                    // else has come along and removed this
                    // value. The GC can do this, for example.
                    // Tht's fine.
                }
            }
        }

        maybe_changed
    }

    fn is_constant(&self, _db: &DB, key: &Q::Key) -> bool {
        let map_read = self.map.read();
        match map_read.get(key) {
            None => false,
            Some(QueryState::InProgress { .. }) => panic!("query in progress"),
            Some(QueryState::Memoized(memo)) => memo.inputs.is_constant(),
        }
    }

    fn keys<C>(&self, _db: &DB) -> C
    where
        C: std::iter::FromIterator<Q::Key>,
    {
        let map = self.map.read();
        map.keys().cloned().collect()
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
        map_write.retain(|key, query_state| {
            match query_state {
                // Leave stuff that is currently being computed -- the
                // other thread doing that work has unique access to
                // this slot and we should not interfere.
                QueryState::InProgress { .. } => {
                    debug!("sweep({:?}({:?})): in-progress", Q::default(), key);
                    true
                }

                // Otherwise, keep only if it was used in this revision.
                QueryState::Memoized(memo) => {
                    debug!(
                        "sweep({:?}({:?})): last verified at {:?}, current revision {:?}",
                        Q::default(),
                        key,
                        memo.verified_at,
                        revision_now
                    );

                    // Since we don't acquire a query lock in this
                    // method, it *is* possible for the revision to
                    // change while we are executing. However, it is
                    // *not* possible for any memos to have been
                    // written into this table that reflect the new
                    // revision, since we are holding the write lock
                    // when we read `revision_now`.
                    assert!(memo.verified_at <= revision_now);

                    if !strategy.keep_values {
                        memo.value = None;
                    }

                    memo.verified_at == revision_now
                }
            }
        });
    }
}

impl<DB, Q, MP> UncheckedMutQueryStorageOps<DB, Q> for DerivedStorage<DB, Q, MP>
where
    Q: QueryFunction<DB>,
    DB: Database,
    MP: MemoizationPolicy<DB, Q>,
{
    fn set_unchecked(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        let key = key.clone();

        let mut map_write = self.map.write();
        let current_revision = db.salsa_runtime().current_revision();
        map_write.insert(
            key,
            QueryState::Memoized(Memo {
                value: Some(value),
                changed_at: current_revision,
                verified_at: current_revision,
                inputs: MemoInputs::Tracked {
                    inputs: Default::default(),
                },
            }),
        );
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
}
