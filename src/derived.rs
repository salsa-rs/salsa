use crate::runtime::ChangedAt;
use crate::runtime::QueryDescriptorSet;
use crate::runtime::Revision;
use crate::runtime::Runtime;
use crate::runtime::RuntimeId;
use crate::runtime::StampedValue;
use crate::CycleDetected;
use crate::Database;
use crate::QueryDescriptor;
use crate::QueryFunction;
use crate::QueryStorageOps;
use crate::UncheckedMutQueryStorageOps;
use log::debug;
use parking_lot::Mutex;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::mpsc::{self, Receiver, Sender};

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

    fn should_track_inputs(key: &Q::Key) -> bool;
}

pub enum AlwaysMemoizeValue {}
impl<DB, Q> MemoizationPolicy<DB, Q> for AlwaysMemoizeValue
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        true
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
    /// Last time the value has actually changed.
    /// changed_at can be less than verified_at.
    changed_at: ChangedAt,

    /// The result of the query, if we decide to memoize it.
    value: Option<Q::Value>,

    /// The inputs that went into our query, if we are tracking them.
    inputs: QueryDescriptorSet<DB>,

    /// Last time that we checked our inputs to see if they have
    /// changed. If this is equal to the current revision, then the
    /// value is up to date. If not, we need to check our inputs and
    /// see if any of them have changed since our last check -- if so,
    /// we'll need to re-execute.
    verified_at: Revision,
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

        let _read_lock = runtime.freeze_revision();

        let revision_now = runtime.current_revision();

        debug!(
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
        let mut old_value = match self.probe(
            self.map.upgradable_read(),
            runtime,
            revision_now,
            descriptor,
            key,
        ) {
            ProbeState::UpToDate(v) => return v,
            ProbeState::StaleOrAbsent(map) => {
                let mut map = RwLockUpgradableReadGuard::upgrade(map);
                map.insert(key.clone(), QueryState::in_progress(runtime.id()))
            }
        };

        // If we have an old-value, it *may* now be stale, since there
        // has been a new revision since the last time we checked. So,
        // first things first, let's walk over each of our previous
        // inputs and check whether they are out of date.
        if let Some(QueryState::Memoized(old_memo)) = &mut old_value {
            if let Some(value) = old_memo.verify_memoized_value(db) {
                debug!("{:?}({:?}): inputs still valid", Q::default(), key);
                // If none of out inputs have changed since the last time we refreshed
                // our value, then our value must still be good. We'll just patch
                // the verified-at date and re-use it.
                old_memo.verified_at = revision_now;
                let changed_at = old_memo.changed_at;

                let new_value = StampedValue { value, changed_at };
                self.overwrite_placeholder(
                    runtime,
                    descriptor,
                    key,
                    old_value.unwrap(),
                    &new_value,
                );
                return Ok(new_value);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let (mut stamped_value, inputs) = runtime.execute_query_implementation(descriptor, || {
            debug!("{:?}({:?}): executing query", Q::default(), key);

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
        if let Some(QueryState::Memoized(old_memo)) = &old_value {
            if old_memo.value.as_ref() == Some(&stamped_value.value) {
                assert!(old_memo.changed_at <= stamped_value.changed_at);
                stamped_value.changed_at = old_memo.changed_at;
            }
        }

        {
            let value = if self.should_memoize_value(key) {
                Some(stamped_value.value.clone())
            } else {
                None
            };
            self.overwrite_placeholder(
                runtime,
                descriptor,
                key,
                QueryState::Memoized(Memo {
                    changed_at: stamped_value.changed_at,
                    value,
                    inputs,
                    verified_at: revision_now,
                }),
                &stamped_value,
            );
        }

        Ok(stamped_value)
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
                debug!(
                    "{:?}({:?}): found memoized value verified_at={:?}",
                    Q::default(),
                    key,
                    memo.verified_at,
                );

                // We've found that the query is definitely up-to-date.
                // If the value is also memoized, return it.
                // Otherwise fallback to recomputing the value.
                if memo.verified_at == revision_now {
                    if let Some(value) = &memo.value {
                        debug!(
                            "{:?}({:?}): returning memoized value (changed_at={:?})",
                            Q::default(),
                            key,
                            memo.changed_at,
                        );
                        return ProbeState::UpToDate(Ok(StampedValue {
                            value: value.clone(),
                            changed_at: memo.changed_at,
                        }));
                    }
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
        map_value: QueryState<DB, Q>,
        new_value: &StampedValue<Q::Value>,
    ) {
        // Overwrite the value, releasing the lock afterwards:
        let waiting = {
            let mut write = self.map.write();
            match write.insert(key.clone(), map_value) {
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

        let descriptors = {
            let map = self.map.read();
            match map.get(key) {
                // If somebody depends on us, but we have no map
                // entry, that must mean that it was found to be out
                // of date and removed.
                None => return true,

                // This value is being actively recomputed. Wait for
                // that thread to finish (assuming it's not dependent
                // on us...) and check its associated revision.
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
                            return value.changed_at.changed_since(revision);
                        }

                        // Consider a cycle to have changed.
                        Err(CycleDetected) => true,
                    };
                }

                Some(QueryState::Memoized(memo)) => {
                    // If our memo is still up to date, then check if we've
                    // changed since the revision.
                    if memo.verified_at == revision_now {
                        return memo.changed_at.changed_since(revision);
                    }

                    // As a special case, if we have no inputs, we are
                    // always clean. No need to update `verified_at`.
                    if let QueryDescriptorSet::Constant = memo.inputs {
                        return false;
                    }

                    // At this point, the value may be dirty (we have
                    // to check the descriptors). If we have a cached
                    // value, we'll just fall back to invoking `read`,
                    // which will do that checking (and a bit more) --
                    // note that we skip the "pure read" part as we
                    // already know the result.
                    if memo.value.is_some() {
                        drop(map);
                        return match self.read_upgrade(db, key, descriptor, revision_now) {
                            Ok(v) => v.changed_at.changed_since(revision),
                            Err(CycleDetected) => true,
                        };
                    }

                    // If there are no inputs or we don't know the
                    // inputs, we can answer right away.
                    match &memo.inputs {
                        QueryDescriptorSet::Constant => return false,
                        QueryDescriptorSet::Untracked => return true,
                        QueryDescriptorSet::Tracked(descriptors) => descriptors.clone(),
                    }
                }
            }
        };

        let maybe_changed = descriptors
            .iter()
            .filter(|descriptor| descriptor.maybe_changed_since(db, revision))
            .inspect(|old_input| {
                debug!(
                    "{:?}({:?}): input `{:?}` may have changed",
                    Q::default(),
                    key,
                    old_input
                )
            })
            .next()
            .is_some();

        // Either way, we have to update our entry.
        {
            let mut map = self.map.write();
            if maybe_changed {
                map.remove(key);
            } else {
                // It is possible that other threads were verifying inputs
                // at the same time. They too will be mutating the
                // map. However, they can only come to the same conclusion
                // that we did.
                match map.get_mut(key) {
                    Some(QueryState::Memoized(memo)) => {
                        memo.verified_at = revision_now;
                    }

                    _ => {
                        panic!("{:?}({:?}) changed state unexpectedly", Q::default(), key,);
                    }
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
            Some(QueryState::Memoized(memo)) => memo.changed_at.is_constant(),
        }
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
        let changed_at = ChangedAt::Revision(current_revision);

        map_write.insert(
            key,
            QueryState::Memoized(Memo {
                value: Some(value),
                changed_at,
                inputs: QueryDescriptorSet::default(),
                verified_at: current_revision,
            }),
        );
    }
}

impl<DB, Q> Memo<DB, Q>
where
    Q: QueryFunction<DB>,
    DB: Database,
{
    fn verify_memoized_value(&self, db: &DB) -> Option<Q::Value> {
        // If we don't have a memoized value, nothing to validate.
        if let Some(v) = &self.value {
            // If inputs are still valid.
            if self.verify_inputs(db) {
                return Some(v.clone());
            }
        }

        None
    }

    fn verify_inputs(&self, db: &DB) -> bool {
        match &self.inputs {
            QueryDescriptorSet::Constant => {
                debug_assert!(match self.changed_at {
                    ChangedAt::Constant(_) => true,
                    ChangedAt::Revision(_) => false,
                });

                true
            }

            QueryDescriptorSet::Tracked(inputs) => {
                debug_assert!(!inputs.is_empty());
                debug_assert!(match self.changed_at {
                    ChangedAt::Constant(_) => false,
                    ChangedAt::Revision(_) => true,
                });

                // Check whether any of our inputs change since the
                // **last point where we were verified** (not since we
                // last changed). This is important: if we have
                // memoized values, then an input may have changed in
                // revision R2, but we found that *our* value was the
                // same regardless, so our change date is still
                // R1. But our *verification* date will be R2, and we
                // are only interested in finding out whether the
                // input changed *again*.
                let changed_input = inputs
                    .iter()
                    .filter(|old_input| old_input.maybe_changed_since(db, self.verified_at))
                    .inspect(|old_input| {
                        debug!(
                            "{:?}::verify_inputs: `{:?}` may have changed",
                            Q::default(),
                            old_input
                        )
                    })
                    .next();

                changed_input.is_none()
            }

            QueryDescriptorSet::Untracked => false,
        }
    }
}
