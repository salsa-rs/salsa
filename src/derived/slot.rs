use crate::debug::TableEntry;
use crate::derived::MemoizationPolicy;
use crate::durability::Durability;
use crate::lru::LruIndex;
use crate::lru::LruNode;
use crate::plumbing::CycleError;
use crate::plumbing::{CycleDetected, CycleRecoveryStrategy};
use crate::plumbing::{DatabaseOps, QueryFunction};
use crate::revision::Revision;
use crate::runtime::Runtime;
use crate::runtime::RuntimeId;
use crate::runtime::StampedValue;
use crate::runtime::WaitResult;
use crate::{Cancelled, Database, DatabaseKeyIndex, Event, EventKind, QueryDb};
use log::{debug, info};
use parking_lot::{RawRwLock, RwLock};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) struct Slot<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    key: Q::Key,
    database_key_index: DatabaseKeyIndex,
    state: RwLock<QueryState<Q>>,
    policy: PhantomData<MP>,
    lru_index: LruIndex,
}

/// Defines the "current state" of query's memoized results.
enum QueryState<Q>
where
    Q: QueryFunction,
{
    NotComputed,

    /// The runtime with the given id is currently computing the
    /// result of this query.
    InProgress {
        id: RuntimeId,

        /// Set to true if any other queries are blocked,
        /// waiting for this query to complete.
        anyone_waiting: AtomicBool,
    },

    /// We have computed the query already, and here is the result.
    Memoized(Memo<Q>),
}

struct Memo<Q>
where
    Q: QueryFunction,
{
    /// The result of the query, if we decide to memoize it.
    value: Option<Q::Value>,

    /// Revision information
    revisions: MemoRevisions,
}

struct MemoRevisions {
    /// Last revision when this memo was verified (if there are
    /// untracked inputs, this will also be when the memo was
    /// created).
    verified_at: Revision,

    /// Last revision when the memoized value was observed to change.
    changed_at: Revision,

    /// Minimum durability of the inputs to this query.
    durability: Durability,

    /// The inputs that went into our query, if we are tracking them.
    inputs: MemoInputs,
}

/// An insertion-order-preserving set of queries. Used to track the
/// inputs accessed during query execution.
pub(super) enum MemoInputs {
    /// Non-empty set of inputs, fully known
    Tracked { inputs: Arc<[DatabaseKeyIndex]> },

    /// Empty set of inputs, fully known.
    NoInputs,

    /// Unknown quantity of inputs
    Untracked,
}

/// Return value of `probe` helper.
enum ProbeState<V, G> {
    UpToDate(Result<V, CycleError>),
    Retry,
    Stale(G),
    Absent(G),
}

impl<Q, MP> Slot<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    pub(super) fn new(key: Q::Key, database_key_index: DatabaseKeyIndex) -> Self {
        Self {
            key,
            database_key_index,
            state: RwLock::new(QueryState::NotComputed),
            lru_index: LruIndex::default(),
            policy: PhantomData,
        }
    }

    pub(super) fn database_key_index(&self) -> DatabaseKeyIndex {
        self.database_key_index
    }

    pub(super) fn read(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
    ) -> Result<StampedValue<Q::Value>, CycleError> {
        let runtime = db.salsa_runtime();

        // NB: We don't need to worry about people modifying the
        // revision out from under our feet. Either `db` is a frozen
        // database, in which case there is a lock, or the mutator
        // thread is the current thread, and it will be prevented from
        // doing any `set` invocations while the query function runs.
        let revision_now = runtime.current_revision();

        info!("{:?}: invoked at {:?}", self, revision_now,);

        // First, do a check with a read-lock.
        loop {
            match self.probe(db, self.state.read(), runtime, revision_now) {
                ProbeState::UpToDate(v) => return v,
                ProbeState::Stale(_) | ProbeState::Absent(_) => break,
                ProbeState::Retry => continue,
            }
        }

        self.read_upgrade(db, revision_now)
    }

    /// Second phase of a read operation: acquires an upgradable-read
    /// and -- if needed -- validates whether inputs have changed,
    /// recomputes value, etc. This is invoked after our initial probe
    /// shows a potentially out of date value.
    fn read_upgrade(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        revision_now: Revision,
    ) -> Result<StampedValue<Q::Value>, CycleError> {
        let runtime = db.salsa_runtime();

        debug!("{:?}: read_upgrade(revision_now={:?})", self, revision_now,);

        // Check with an upgradable read to see if there is a value
        // already. (This permits other readers but prevents anyone
        // else from running `read_upgrade` at the same time.)
        let old_memo = loop {
            match self.probe(db, self.state.upgradable_read(), runtime, revision_now) {
                ProbeState::UpToDate(v) => return v,
                ProbeState::Stale(state) | ProbeState::Absent(state) => {
                    type RwLockUpgradableReadGuard<'a, T> =
                        lock_api::RwLockUpgradableReadGuard<'a, RawRwLock, T>;

                    let mut state = RwLockUpgradableReadGuard::upgrade(state);
                    match std::mem::replace(&mut *state, QueryState::in_progress(runtime.id())) {
                        QueryState::Memoized(old_memo) => break Some(old_memo),
                        QueryState::InProgress { .. } => unreachable!(),
                        QueryState::NotComputed => break None,
                    }
                }
                ProbeState::Retry => continue,
            }
        };

        let mut panic_guard = PanicGuard::new(self.database_key_index, self, old_memo, runtime);

        // If we have an old-value, it *may* now be stale, since there
        // has been a new revision since the last time we checked. So,
        // first things first, let's walk over each of our previous
        // inputs and check whether they are out of date.
        if let Some(memo) = &mut panic_guard.memo {
            if let Some(value) = memo.validate_memoized_value(db, revision_now) {
                info!("{:?}: validated old memoized value", self,);

                db.salsa_event(Event {
                    runtime_id: runtime.id(),
                    kind: EventKind::DidValidateMemoizedValue {
                        database_key: self.database_key_index,
                    },
                });

                panic_guard.proceed();

                return Ok(value);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let mut result = runtime.execute_query_implementation(db, self.database_key_index, || {
            info!("{:?}: executing query", self);

            Q::execute(db, self.key.clone())
        });

        if let Some(cycle) = &result.cycle {
            result.value = Q::cycle_fallback(db, cycle, &self.key);
        }

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
                // Careful: if the value became less durable than it
                // used to be, that is a "breaking change" that our
                // consumers must be aware of. Becoming *more* durable
                // is not. See the test `constant_to_non_constant`.
                if result.durability >= old_memo.revisions.durability
                    && MP::memoized_value_eq(&old_value, &result.value)
                {
                    debug!(
                        "read_upgrade({:?}): value is equal, back-dating to {:?}",
                        self, old_memo.revisions.changed_at,
                    );

                    assert!(old_memo.revisions.changed_at <= result.changed_at);
                    result.changed_at = old_memo.revisions.changed_at;
                }
            }
        }

        let new_value = StampedValue {
            value: result.value,
            durability: result.durability,
            changed_at: result.changed_at,
        };

        let value = if self.should_memoize_value(&self.key) {
            Some(new_value.value.clone())
        } else {
            None
        };

        debug!(
            "read_upgrade({:?}): result.changed_at={:?}, \
             result.durability={:?}, result.dependencies = {:?}",
            self, result.changed_at, result.durability, result.dependencies,
        );

        let inputs = match result.dependencies {
            None => MemoInputs::Untracked,

            Some(dependencies) => {
                if dependencies.is_empty() {
                    MemoInputs::NoInputs
                } else {
                    MemoInputs::Tracked {
                        inputs: dependencies.into_iter().collect(),
                    }
                }
            }
        };
        debug!("read_upgrade({:?}): inputs={:#?}", self, inputs.debug(db));

        panic_guard.memo = Some(Memo {
            value,
            revisions: MemoRevisions {
                changed_at: result.changed_at,
                verified_at: revision_now,
                inputs,
                durability: result.durability,
            },
        });

        panic_guard.proceed();

        Ok(new_value)
    }

    /// Helper for `read` that does a shallow check (not recursive) if we have an up-to-date value.
    ///
    /// Invoked with the guard `state` corresponding to the `QueryState` of some `Slot` (the guard
    /// can be either read or write). Returns a suitable `ProbeState`:
    ///
    /// - `ProbeState::UpToDate(r)` if the table has an up-to-date value (or we blocked on another
    ///   thread that produced such a value).
    /// - `ProbeState::StaleOrAbsent(g)` if either (a) there is no memo for this key, (b) the memo
    ///   has no value; or (c) the memo has not been verified at the current revision.
    ///
    /// Note that in case `ProbeState::UpToDate`, the lock will have been released.
    fn probe<StateGuard>(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        state: StateGuard,
        runtime: &Runtime,
        revision_now: Revision,
    ) -> ProbeState<StampedValue<Q::Value>, StateGuard>
    where
        StateGuard: Deref<Target = QueryState<Q>>,
    {
        match &*state {
            QueryState::NotComputed => ProbeState::Absent(state),

            QueryState::InProgress { id, anyone_waiting } => {
                let other_id = *id;

                // NB: `Ordering::Relaxed` is sufficient here,
                // as there are no loads that are "gated" on this
                // value. Everything that is written is also protected
                // by a lock that must be acquired. The role of this
                // boolean is to decide *whether* to acquire the lock,
                // not to gate future atomic reads.
                anyone_waiting.store(true, Ordering::Relaxed);

                return match self.block_on_in_progress_thread(db, runtime, other_id, state) {
                    Ok(WaitResult::Panicked) => Cancelled::throw(),
                    Ok(WaitResult::Completed) => ProbeState::Retry,
                    Err(err) => ProbeState::UpToDate(
                        match runtime.report_unexpected_cycle(
                            db.ops_database(),
                            self.database_key_index,
                            err,
                            revision_now,
                        ) {
                            (CycleRecoveryStrategy::Panic, err) => Err(err),
                            (CycleRecoveryStrategy::Fallback, err) => Ok(StampedValue {
                                value: Q::cycle_fallback(db, &err.cycle, &self.key),
                                changed_at: err.changed_at,
                                durability: err.durability,
                            }),
                        },
                    ),
                };
            }

            QueryState::Memoized(memo) => {
                debug!(
                    "{:?}: found memoized value, verified_at={:?}, changed_at={:?}",
                    self, memo.revisions.verified_at, memo.revisions.changed_at,
                );

                if memo.revisions.verified_at < revision_now {
                    return ProbeState::Stale(state);
                }

                if let Some(value) = &memo.value {
                    let value = StampedValue {
                        durability: memo.revisions.durability,
                        changed_at: memo.revisions.changed_at,
                        value: value.clone(),
                    };

                    info!(
                        "{:?}: returning memoized value changed at {:?}",
                        self, value.changed_at
                    );

                    ProbeState::UpToDate(Ok(value))
                } else {
                    ProbeState::Absent(state)
                }
            }
        }
    }

    pub(super) fn durability(&self, db: &<Q as QueryDb<'_>>::DynDb) -> Durability {
        match &*self.state.read() {
            QueryState::NotComputed => Durability::LOW,
            QueryState::InProgress { .. } => panic!("query in progress"),
            QueryState::Memoized(memo) => {
                if memo.revisions.check_durability(db.salsa_runtime()) {
                    memo.revisions.durability
                } else {
                    Durability::LOW
                }
            }
        }
    }

    pub(super) fn as_table_entry(&self) -> Option<TableEntry<Q::Key, Q::Value>> {
        match &*self.state.read() {
            QueryState::NotComputed => None,
            QueryState::InProgress { .. } => Some(TableEntry::new(self.key.clone(), None)),
            QueryState::Memoized(memo) => {
                Some(TableEntry::new(self.key.clone(), memo.value.clone()))
            }
        }
    }

    pub(super) fn evict(&self) {
        let mut state = self.state.write();
        if let QueryState::Memoized(memo) = &mut *state {
            // Evicting a value with an untracked input could
            // lead to inconsistencies. Note that we can't check
            // `has_untracked_input` when we add the value to the cache,
            // because inputs can become untracked in the next revision.
            if memo.revisions.has_untracked_input() {
                return;
            }
            memo.value = None;
        }
    }

    pub(super) fn invalidate(&self, new_revision: Revision) -> Option<Durability> {
        log::debug!("Slot::invalidate(new_revision = {:?})", new_revision);
        match &mut *self.state.write() {
            QueryState::Memoized(memo) => {
                memo.revisions.inputs = MemoInputs::Untracked;
                memo.revisions.changed_at = new_revision;
                Some(memo.revisions.durability)
            }
            QueryState::NotComputed => None,
            QueryState::InProgress { .. } => unreachable!(),
        }
    }

    pub(super) fn maybe_changed_since(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        revision: Revision,
    ) -> bool {
        let runtime = db.salsa_runtime();
        let revision_now = runtime.current_revision();

        db.unwind_if_cancelled();

        debug!(
            "maybe_changed_since({:?}) called with revision={:?}, revision_now={:?}",
            self, revision, revision_now,
        );

        // Acquire read lock to start. In some of the arms below, we
        // drop this explicitly.
        let state = self.state.read();

        // Look for a memoized value.
        let memo = match &*state {
            // If somebody depends on us, but we have no map
            // entry, that must mean that it was found to be out
            // of date and removed.
            QueryState::NotComputed => {
                debug!("maybe_changed_since({:?}): no value", self);
                return true;
            }

            // This value is being actively recomputed. Wait for
            // that thread to finish (assuming it's not dependent
            // on us...) and check its associated revision.
            QueryState::InProgress { id, anyone_waiting } => {
                let other_id = *id;
                debug!(
                    "maybe_changed_since({:?}): blocking on thread `{:?}`",
                    self, other_id,
                );

                // NB: `Ordering::Relaxed` is sufficient here,
                // see `probe` for more details.
                anyone_waiting.store(true, Ordering::Relaxed);

                return match self.block_on_in_progress_thread(db, runtime, other_id, state) {
                    // The other thread has completed. Have to try again. We've lost our lock,
                    // so just recurse. (We should probably clean this up to a loop later,
                    // but recursing is not terrible: this shouldn't happen more than once per revision.)
                    Ok(WaitResult::Completed) => self.maybe_changed_since(db, revision),
                    Ok(WaitResult::Panicked) => Cancelled::throw(),
                    Err(_) => true,
                };
            }

            QueryState::Memoized(memo) => memo,
        };

        if memo.revisions.verified_at == revision_now {
            debug!(
                "maybe_changed_since({:?}): {:?} since up-to-date memo that changed at {:?}",
                self,
                memo.revisions.changed_at > revision,
                memo.revisions.changed_at,
            );
            return memo.revisions.changed_at > revision;
        }

        let maybe_changed;

        // If we only depended on constants, and no constant has been
        // modified since then, we cannot have changed; no need to
        // trace our inputs.
        if memo.revisions.check_durability(runtime) {
            std::mem::drop(state);
            maybe_changed = false;
        } else {
            match &memo.revisions.inputs {
                MemoInputs::Untracked => {
                    // we don't know the full set of
                    // inputs, so if there is a new
                    // revision, we must assume it is
                    // dirty
                    debug!(
                        "maybe_changed_since({:?}: true since untracked inputs",
                        self,
                    );
                    return true;
                }

                MemoInputs::NoInputs => {
                    std::mem::drop(state);
                    maybe_changed = false;
                }

                MemoInputs::Tracked { inputs } => {
                    // At this point, the value may be dirty (we have
                    // to check the database-keys). If we have a cached
                    // value, we'll just fall back to invoking `read`,
                    // which will do that checking (and a bit more) --
                    // note that we skip the "pure read" part as we
                    // already know the result.
                    assert!(inputs.len() > 0);
                    if memo.value.is_some() {
                        std::mem::drop(state);
                        return match self.read_upgrade(db, revision_now) {
                            Ok(v) => {
                                debug!(
                                    "maybe_changed_since({:?}: {:?} since (recomputed) value changed at {:?}",
                                    self,
                                    v.changed_at > revision,
                                    v.changed_at,
                                );
                                v.changed_at > revision
                            }
                            Err(_) => true,
                        };
                    }

                    // We have a **tracked set of inputs** that need to be validated.
                    let inputs = inputs.clone();
                    // We'll need to update the state anyway (see below), so release the read-lock.
                    std::mem::drop(state);

                    // Iterate the inputs and see if any have maybe changed.
                    maybe_changed = inputs
                        .iter()
                        .filter(|&&input| db.maybe_changed_since(input, revision))
                        .inspect(|input| debug!("{:?}: input `{:?}` may have changed", self, input))
                        .next()
                        .is_some();
                }
            }
        }

        // Either way, we have to update our entry.
        //
        // Keep in mind, though, that we released the lock before checking the ipnuts and a lot
        // could have happened in the interim. =) Therefore, we have to probe the current
        // `self.state`  again and in some cases we ought to do nothing.
        {
            let mut state = self.state.write();
            match &mut *state {
                QueryState::Memoized(memo) => {
                    if memo.revisions.verified_at == revision_now {
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
                        *state = QueryState::NotComputed;
                    } else {
                        // We found this entry is valid. Update the
                        // `verified_at` to reflect the current
                        // revision.
                        memo.revisions.verified_at = revision_now;
                    }
                }

                QueryState::InProgress { .. } => {
                    // Since we started verifying inputs, somebody
                    // else has come along and started updated this
                    // value. Just leave their marker alone and return
                    // whatever `maybe_changed` value we computed.
                }

                QueryState::NotComputed => {
                    // Since we started verifying inputs, somebody
                    // else has come along and removed this value. The
                    // GC can do this, for example. That's fine.
                }
            }
        }

        maybe_changed
    }

    /// Helper:
    ///
    /// When we encounter an `InProgress` indicator, we need to either
    /// report a cycle or else register ourselves to be notified when
    /// that work completes. This helper does that. If a cycle is detected,
    /// it returns immediately, but otherwise it blocks `self.database_key_index`
    /// has completed and returns the corresponding result.
    fn block_on_in_progress_thread<MutexGuard>(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        runtime: &Runtime,
        other_id: RuntimeId,
        mutex_guard: MutexGuard,
    ) -> Result<WaitResult, CycleDetected> {
        runtime.try_block_on(
            db.ops_database(),
            self.database_key_index,
            other_id,
            mutex_guard,
        )
    }

    fn should_memoize_value(&self, key: &Q::Key) -> bool {
        MP::should_memoize_value(key)
    }
}

impl<Q> QueryState<Q>
where
    Q: QueryFunction,
{
    fn in_progress(id: RuntimeId) -> Self {
        QueryState::InProgress {
            id,
            anyone_waiting: Default::default(),
        }
    }
}

struct PanicGuard<'me, Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    database_key_index: DatabaseKeyIndex,
    slot: &'me Slot<Q, MP>,
    memo: Option<Memo<Q>>,
    runtime: &'me Runtime,
}

impl<'me, Q, MP> PanicGuard<'me, Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn new(
        database_key_index: DatabaseKeyIndex,
        slot: &'me Slot<Q, MP>,
        memo: Option<Memo<Q>>,
        runtime: &'me Runtime,
    ) -> Self {
        Self {
            database_key_index,
            slot,
            memo,
            runtime,
        }
    }

    /// Proceed with our panic guard by overwriting the placeholder for `key`.
    /// Once that completes, ensure that our deconstructor is not run once we
    /// are out of scope.
    fn proceed(mut self) {
        self.overwrite_placeholder(WaitResult::Completed);
        std::mem::forget(self)
    }

    /// Overwrites the `InProgress` placeholder for `key` that we
    /// inserted; if others were blocked, waiting for us to finish,
    /// then notify them.
    fn overwrite_placeholder(&mut self, wait_result: WaitResult) {
        let mut write = self.slot.state.write();

        let old_value = match self.memo.take() {
            // Replace the `InProgress` marker that we installed with the new
            // memo, thus releasing our unique access to this key.
            Some(memo) => std::mem::replace(&mut *write, QueryState::Memoized(memo)),

            // We had installed an `InProgress` marker, but we panicked before
            // it could be removed. At this point, we therefore "own" unique
            // access to our slot, so we can just remove the key.
            None => std::mem::replace(&mut *write, QueryState::NotComputed),
        };

        match old_value {
            QueryState::InProgress { id, anyone_waiting } => {
                assert_eq!(id, self.runtime.id());

                // NB: As noted on the `store`, `Ordering::Relaxed` is
                // sufficient here. This boolean signals us on whether to
                // acquire a mutex; the mutex will guarantee that all writes
                // we are interested in are visible.
                if anyone_waiting.load(Ordering::Relaxed) {
                    self.runtime
                        .unblock_queries_blocked_on(self.database_key_index, wait_result);
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

impl<'me, Q, MP> Drop for PanicGuard<'me, Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn drop(&mut self) {
        if std::thread::panicking() {
            // We panicked before we could proceed and need to remove `key`.
            self.overwrite_placeholder(WaitResult::Panicked)
        } else {
            // If no panic occurred, then panic guard ought to be
            // "forgotten" and so this Drop code should never run.
            panic!(".forget() was not called")
        }
    }
}

impl<Q> Memo<Q>
where
    Q: QueryFunction,
{
    fn validate_memoized_value(
        &mut self,
        db: &<Q as QueryDb<'_>>::DynDb,
        revision_now: Revision,
    ) -> Option<StampedValue<Q::Value>> {
        // If we don't have a memoized value, nothing to validate.
        let value = match &self.value {
            None => return None,
            Some(v) => v,
        };

        let dyn_db = db.ops_database();
        if self.revisions.validate_memoized_value(dyn_db, revision_now) {
            Some(StampedValue {
                durability: self.revisions.durability,
                changed_at: self.revisions.changed_at,
                value: value.clone(),
            })
        } else {
            None
        }
    }
}

impl MemoRevisions {
    fn validate_memoized_value(&mut self, db: &dyn Database, revision_now: Revision) -> bool {
        assert!(self.verified_at != revision_now);
        let verified_at = self.verified_at;

        debug!(
            "validate_memoized_value: verified_at={:?}, revision_now={:?}, inputs={:#?}",
            verified_at, revision_now, self.inputs
        );

        if self.check_durability(db.salsa_runtime()) {
            return self.mark_value_as_verified(revision_now);
        }

        match &self.inputs {
            // We can't validate values that had untracked inputs; just have to
            // re-execute.
            MemoInputs::Untracked => {
                return false;
            }

            MemoInputs::NoInputs => {}

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
                    .find(|&&input| db.maybe_changed_since(input, verified_at));
                if let Some(input) = changed_input {
                    debug!("validate_memoized_value: `{:?}` may have changed", input);

                    return false;
                }
            }
        };

        self.mark_value_as_verified(revision_now)
    }

    /// True if this memo is known not to have changed based on its durability.
    fn check_durability(&self, runtime: &Runtime) -> bool {
        let last_changed = runtime.last_changed_revision(self.durability);
        debug!(
            "check_durability(last_changed={:?} <= verified_at={:?}) = {:?}",
            last_changed,
            self.verified_at,
            last_changed <= self.verified_at,
        );
        last_changed <= self.verified_at
    }

    fn mark_value_as_verified(&mut self, revision_now: Revision) -> bool {
        self.verified_at = revision_now;
        true
    }

    fn has_untracked_input(&self) -> bool {
        matches!(self.inputs, MemoInputs::Untracked)
    }
}

impl<Q, MP> std::fmt::Debug for Slot<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{:?}({:?})", Q::default(), self.key)
    }
}

impl MemoInputs {
    fn debug<'a, D: ?Sized>(&'a self, db: &'a D) -> impl std::fmt::Debug + 'a
    where
        D: DatabaseOps,
    {
        enum DebugMemoInputs<'a, D: ?Sized> {
            Tracked {
                inputs: &'a [DatabaseKeyIndex],
                db: &'a D,
            },
            NoInputs,
            Untracked,
        }

        impl<D: ?Sized + DatabaseOps> std::fmt::Debug for DebugMemoInputs<'_, D> {
            fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    DebugMemoInputs::Tracked { inputs, db } => fmt
                        .debug_struct("Tracked")
                        .field(
                            "inputs",
                            &inputs.iter().map(|key| key.debug(*db)).collect::<Vec<_>>(),
                        )
                        .finish(),
                    DebugMemoInputs::NoInputs => fmt.debug_struct("NoInputs").finish(),
                    DebugMemoInputs::Untracked => fmt.debug_struct("Untracked").finish(),
                }
            }
        }

        match self {
            MemoInputs::Tracked { inputs } => DebugMemoInputs::Tracked {
                inputs: &inputs,
                db,
            },
            MemoInputs::NoInputs => DebugMemoInputs::NoInputs,
            MemoInputs::Untracked => DebugMemoInputs::Untracked,
        }
    }
}

impl std::fmt::Debug for MemoInputs {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoInputs::Tracked { inputs } => {
                fmt.debug_struct("Tracked").field("inputs", inputs).finish()
            }
            MemoInputs::NoInputs => fmt.debug_struct("NoInputs").finish(),
            MemoInputs::Untracked => fmt.debug_struct("Untracked").finish(),
        }
    }
}

impl<Q, MP> LruNode for Slot<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn lru_index(&self) -> &LruIndex {
        &self.lru_index
    }
}

/// Check that `Slot<Q, MP>: Send + Sync` as long as
/// `DB::DatabaseData: Send + Sync`, which in turn implies that
/// `Q::Key: Send + Sync`, `Q::Value: Send + Sync`.
#[allow(dead_code)]
fn check_send_sync<Q, MP>()
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
    Q::Key: Send + Sync,
    Q::Value: Send + Sync,
{
    fn is_send_sync<T: Send + Sync>() {}
    is_send_sync::<Slot<Q, MP>>();
}

/// Check that `Slot<Q, MP>: 'static` as long as
/// `DB::DatabaseData: 'static`, which in turn implies that
/// `Q::Key: 'static`, `Q::Value: 'static`.
#[allow(dead_code)]
fn check_static<Q, MP>()
where
    Q: QueryFunction + 'static,
    MP: MemoizationPolicy<Q> + 'static,
    Q::Key: 'static,
    Q::Value: 'static,
{
    fn is_static<T: 'static>() {}
    is_static::<Slot<Q, MP>>();
}
