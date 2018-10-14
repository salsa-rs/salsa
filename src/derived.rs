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
use parking_lot::Condvar;
use parking_lot::Mutex;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};

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

    /// This cond var is used when one thread is waiting on another to
    /// produce some specific key. In that case, the thread producing
    /// the key will signal the cond-var. The threads awaiting the key
    /// will check in `map` to see if their key is present and (if
    /// not) await the cond-var.
    signal_cond_var: Condvar,

    /// Mutex used for `signal_cond_var`. Note that this mutex is
    /// never acquired while holding the lock on `map` (but you may
    /// acquire the `map` lock while holding this mutex).
    signal_mutex: Mutex<()>,
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
        others_waiting: AtomicBool,
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
            others_waiting: AtomicBool::new(false),
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
            signal_cond_var: Default::default(),
            signal_mutex: Default::default(),
        }
    }
}

/// Return value of `probe` helper.
enum ProbeState<V, G> {
    UpToDate(V),
    CycleDetected,
    StaleOrAbsent(G),
    BlockedOnOtherThread,
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

        // In this loop, we are looking for an up-to-date value. The loop is needed
        // to handle other threads: if we find that some other thread is "using" this
        // key, we will block until they are done and then loop back around and try
        // again.
        //
        // Otherwise, we first check for a usable value with the read
        // lock. If that fails, we acquire the write lock and try
        // again. We don't use an upgradable read lock because that
        // would eliminate the ability for multiple cache hits to be
        // executing in parallel.
        let mut old_value = loop {
            // Read-lock check.
            match self.read_probe(self.map.read(), runtime, revision_now, descriptor, key) {
                ProbeState::UpToDate(v) => return Ok(v),
                ProbeState::CycleDetected => return Err(CycleDetected),
                ProbeState::BlockedOnOtherThread => {
                    continue;
                }
                ProbeState::StaleOrAbsent(_guard) => (),
            }

            // Write-lock check: install `InProgress` sentinel if no usable value.
            match self.read_probe(self.map.write(), runtime, revision_now, descriptor, key) {
                ProbeState::UpToDate(v) => return Ok(v),
                ProbeState::CycleDetected => return Err(CycleDetected),
                ProbeState::BlockedOnOtherThread => {
                    continue;
                }
                ProbeState::StaleOrAbsent(mut map) => {
                    break map.insert(key.clone(), QueryState::in_progress(runtime.id()))
                }
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

                self.overwrite_placeholder(runtime, descriptor, key, old_value.unwrap());
                return Ok(StampedValue { value, changed_at });
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
            );
        }

        Ok(stamped_value)
    }

    /// Helper for `read`:
    ///
    /// Looks in the map to see if we have an up-to-date value or a
    /// cycle. If so, returns `Ok(v)` with either the value or a cycle-error;
    /// this can be propagated as the final result of read.
    ///
    /// Otherwise, returns `Err(map)` where `map` is the lock guard
    /// that was given in as argument.
    fn read_probe<MapGuard>(
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
        self.probe(map, runtime, revision_now, descriptor, key, |memo| {
            if let Some(value) = &memo.value {
                debug!(
                    "{:?}({:?}): returning memoized value (changed_at={:?})",
                    Q::default(),
                    key,
                    memo.changed_at,
                );
                Some(StampedValue {
                    value: value.clone(),
                    changed_at: memo.changed_at,
                })
            } else {
                None
            }
        })
    }

    /// Helper:
    ///
    /// Invoked with the guard `map` of some lock on `self.map` (read
    /// or write) as well as details about the key to look up. It will
    /// check the map and return a suitable `ProbeState`:
    ///
    /// - `ProbeState::UpToDate(r)` if the memo is up-to-date,
    ///   and invoking `with_up_to_date_memo` returned `Some(r)`.
    /// - `ProbeState::CycleDetected` if this thread is (directly or
    ///   indirectly) already computing this value.
    /// - `ProbeState::BlockedOnOtherThread` if some other thread
    ///   (which does not depend on us) was already computing this
    ///   value; caller should re-acquire the lock and try again.
    /// - `ProbeState::StaleOrAbsent` if either (a) there is no memo for this key,
    ///    (b) the memo has not been verified at the current revision, or
    ///    (c) `with_up_to_date_memo` returned `None`.
    ///
    /// Note that in all cases **except** for `StaleOrAbsent`, the lock on
    /// `map` will have been released.
    fn probe<MapGuard, R>(
        &self,
        map: MapGuard,
        runtime: &Runtime<DB>,
        revision_now: Revision,
        descriptor: &DB::QueryDescriptor,
        key: &Q::Key,
        with_up_to_date_memo: impl FnOnce(&Memo<DB, Q>) -> Option<R>,
    ) -> ProbeState<R, MapGuard>
    where
        MapGuard: Deref<Target = FxHashMap<Q::Key, QueryState<DB, Q>>>,
    {
        match map.get(key) {
            Some(QueryState::InProgress { id, others_waiting }) => {
                let other_id = *id;
                if other_id == runtime.id() {
                    return ProbeState::CycleDetected;
                } else {
                    if !runtime.try_block_on(descriptor, other_id) {
                        return ProbeState::CycleDetected;
                    }

                    // The reader of this will have to acquire map
                    // lock, we don't need any particular ordering.
                    others_waiting.store(true, Ordering::Relaxed);

                    // Release our lock on `self.map`, so other thread
                    // can complete.
                    std::mem::drop(map);

                    // Wait for other thread to overwrite this placeholder.
                    self.await_other_thread(other_id, key);

                    return ProbeState::BlockedOnOtherThread;
                }
            }

            Some(QueryState::Memoized(m)) => {
                debug!(
                    "{:?}({:?}): found memoized value verified_at={:?}",
                    Q::default(),
                    key,
                    m.verified_at,
                );

                // We've found that the query is definitely up-to-date.
                // If the value is also memoized, return it.
                // Otherwise fallback to recomputing the value.
                if m.verified_at == revision_now {
                    if let Some(r) = with_up_to_date_memo(&m) {
                        return ProbeState::UpToDate(r);
                    }
                }
            }

            None => {}
        }

        ProbeState::StaleOrAbsent(map)
    }

    /// If some other thread is tasked with producing a memoized
    /// result for this value, then wait for them.
    ///
    /// Pre-conditions:
    /// - we have installed ourselves in the dependency graph and set the
    ///   bool that informs the producer we are waiting
    /// - `self.map` must not be locked
    fn await_other_thread(&self, other_id: RuntimeId, key: &Q::Key) {
        let mut signal_lock_guard = self.signal_mutex.lock();

        loop {
            {
                let map = self.map.read();

                match map.get(key) {
                    Some(QueryState::InProgress {
                        id,
                        others_waiting: _,
                    }) => {
                        // Other thread still working!
                        assert_eq!(*id, other_id);
                    }

                    _ => {
                        // The other thread finished!
                        return;
                    }
                }
            }

            self.signal_cond_var.wait(&mut signal_lock_guard);
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
        value: QueryState<DB, Q>,
    ) {
        // Overwrite the value, releasing the lock afterwards:
        {
            let mut write = self.map.write();
            match write.insert(key.clone(), value) {
                Some(QueryState::InProgress { id, others_waiting }) => {
                    assert_eq!(id, runtime.id());

                    // Others only write to this while holding the
                    // read-lock, and we have the write lock, so they
                    // must all have released their locks before we
                    // acquired ours. Therefore, we see their writes and
                    // can use relaxed ordering.
                    let others_waiting = others_waiting.load(Ordering::Relaxed);
                    if !others_waiting {
                        // if nobody is waiting, we are done here
                        return;
                    }

                    runtime.unblock_queries_blocked_on_self(descriptor);
                }

                _ => panic!("expected in-progress state"),
            }
        }

        // Now, with the lock released, notify the others that they
        // can unblock themselves. It is imp't that we acquire the
        // signal-mutex-lock, because others will also be acquiring it
        // to ensure that their "check the map" and "await" happens
        // atomically with respect to our notify.
        let _signal_lock_guard = self.signal_mutex.lock();
        self.signal_cond_var.notify_all();
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

        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?}, revision_now={:?})",
            Q::default(),
            key,
            revision,
            revision_now,
        );

        let value = {
            let map_read = self.map.upgradable_read();
            match map_read.get(key) {
                None | Some(QueryState::InProgress { .. }) => return true,
                Some(QueryState::Memoized(memo)) => {
                    // If our memo is still up to date, then check if we've
                    // changed since the revision.
                    if memo.verified_at == revision_now {
                        return memo.changed_at.changed_since(revision);
                    }
                    if memo.value.is_some() {
                        // Otherwise, if we cache values, fall back to the full read to compute the result.
                        drop(memo);
                        drop(map_read);
                        return match self.read(db, key, descriptor) {
                            Ok(v) => v.changed_at.changed_since(revision),
                            Err(CycleDetected) => true,
                        };
                    }
                }
            };
            // If, however, we don't cache values, then optimistically
            // try to advance `verified_at` by walking the inputs.
            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::in_progress(runtime.id()))
        };

        let mut memo = match value {
            Some(QueryState::Memoized(memo)) => memo,
            _ => unreachable!(),
        };

        if memo.verify_inputs(db) {
            memo.verified_at = revision_now;
            self.overwrite_placeholder(runtime, descriptor, key, QueryState::Memoized(memo));
            return false;
        }

        // Just remove the existing entry. It's out of date.
        self.map.write().remove(key);

        true
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
        match self.changed_at {
            ChangedAt::Constant(_) => {
                // If we know that the value is constant, it had
                // better not change, but in that case, we ought not
                // to have any inputs. Using `debug_assert` because
                // this is on the fast path.
                debug_assert!(match &self.inputs {
                    QueryDescriptorSet::Tracked(inputs) => inputs.is_empty(),
                    QueryDescriptorSet::Untracked => false,
                });

                true
            }

            ChangedAt::Revision(revision) => match &self.inputs {
                QueryDescriptorSet::Tracked(inputs) => inputs
                    .iter()
                    .all(|old_input| !old_input.maybe_changed_since(db, revision)),

                QueryDescriptorSet::Untracked => false,
            },
        }
    }
}
