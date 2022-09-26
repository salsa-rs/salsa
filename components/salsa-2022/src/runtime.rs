use std::{
    panic::panic_any,
    sync::{atomic::Ordering, Arc},
};

use crate::{
    cycle::CycleRecoveryStrategy,
    debug::DebugWithDb,
    durability::Durability,
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::active_query::ActiveQuery,
    Cancelled, Cycle, Database, Event, EventKind, Revision,
};

use self::{
    dependency_graph::DependencyGraph,
    local_state::{ActiveQueryGuard, EdgeKind},
};

use super::{tracked_struct::Disambiguator, IngredientIndex};

mod active_query;
mod dependency_graph;
pub mod local_state;
mod shared_state;

pub struct Runtime {
    /// Our unique runtime id.
    id: RuntimeId,

    /// Local state that is specific to this runtime (thread).
    local_state: local_state::LocalState,

    /// Shared state that is accessible via all runtimes.
    shared_state: Arc<shared_state::SharedState>,
}

#[derive(Clone, Debug)]
pub(crate) enum WaitResult {
    Completed,
    Panicked,
    Cycle(Cycle),
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

impl<V> StampedValue<V> {
    // FIXME: Use or remove this.
    #[allow(dead_code)]
    pub(crate) fn merge_revision_info<U>(&mut self, other: &StampedValue<U>) {
        self.durability = self.durability.min(other.durability);
        self.changed_at = self.changed_at.max(other.changed_at);
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            id: RuntimeId { counter: 0 },
            shared_state: Default::default(),
            local_state: Default::default(),
        }
    }
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Runtime")
            .field("id", &self.id())
            .field("shared_state", &self.shared_state)
            .finish()
    }
}

impl Runtime {
    pub(crate) fn id(&self) -> RuntimeId {
        self.id
    }

    pub(crate) fn current_revision(&self) -> Revision {
        self.shared_state.revisions[0].load()
    }

    /// Returns the index of the active query along with its *current* durability/changed-at
    /// information. As the query continues to execute, naturally, that information may change.
    pub(crate) fn active_query(&self) -> Option<(DatabaseKeyIndex, StampedValue<()>)> {
        self.local_state.active_query()
    }

    pub(crate) fn empty_dependencies(&self) -> Arc<[(EdgeKind, DependencyIndex)]> {
        self.shared_state.empty_dependencies.clone()
    }

    pub fn snapshot(&self) -> Self {
        if self.local_state.query_in_progress() {
            panic!("it is not legal to `snapshot` during a query (see salsa-rs/salsa#80)");
        }

        let id = RuntimeId {
            counter: self.shared_state.next_id.fetch_add(1, Ordering::SeqCst),
        };

        Runtime {
            id,
            shared_state: self.shared_state.clone(),
            local_state: Default::default(),
        }
    }

    pub(crate) fn report_tracked_read(
        &self,
        key_index: DependencyIndex,
        durability: Durability,
        changed_at: Revision,
    ) {
        self.local_state
            .report_tracked_read(key_index, durability, changed_at)
    }

    /// Reports that the query depends on some state unknown to salsa.
    ///
    /// Queries which report untracked reads will be re-executed in the next
    /// revision.
    pub fn report_untracked_read(&self) {
        self.local_state
            .report_untracked_read(self.current_revision());
    }

    /// Reports that an input with durability `durability` changed.
    /// This will update the 'last changed at' values for every durability
    /// less than or equal to `durability` to the current revision.
    pub(crate) fn report_tracked_write(&mut self, durability: Durability) {
        let new_revision = self.current_revision();
        for rev in &self.shared_state.revisions[1..=durability.index()] {
            rev.store(new_revision);
        }
    }

    /// Adds `key` to the list of output created by the current query
    /// (if not already present).
    pub(crate) fn add_output(&self, key: DependencyIndex) {
        self.local_state.add_output(key);
    }

    /// Check whether `entity` is contained the list of outputs written by the current query.
    pub(super) fn is_output_of_active_query(&self, entity: DependencyIndex) -> bool {
        self.local_state.is_output(entity)
    }

    /// Called when the active queries creates an index from the
    /// entity table with the index `entity_index`. Has the following effects:
    ///
    /// * Add a query read on `DatabaseKeyIndex::for_table(entity_index)`
    /// * Identify a unique disambiguator for the hash within the current query,
    ///   adding the hash to the current query's disambiguator table.
    /// * Return that hash + id of the current query.
    pub(crate) fn disambiguate_entity(
        &self,
        entity_index: IngredientIndex,
        reset_at: Revision,
        data_hash: u64,
    ) -> (DatabaseKeyIndex, Disambiguator) {
        self.report_tracked_read(
            DependencyIndex::for_table(entity_index),
            Durability::MAX,
            reset_at,
        );
        self.local_state.disambiguate(data_hash)
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

    /// Starts unwinding the stack if the current revision is cancelled.
    ///
    /// This method can be called by query implementations that perform
    /// potentially expensive computations, in order to speed up propagation of
    /// cancellation.
    ///
    /// Cancellation will automatically be triggered by salsa on any query
    /// invocation.
    ///
    /// This method should not be overridden by `Database` implementors. A
    /// `salsa_event` is emitted when this method is called, so that should be
    /// used instead.
    pub(crate) fn unwind_if_revision_cancelled<DB: ?Sized + Database>(&self, db: &DB) {
        db.salsa_event(Event {
            runtime_id: self.id(),
            kind: EventKind::WillCheckCancellation,
        });
        if self.shared_state.revision_canceled.load() {
            db.salsa_event(Event {
                runtime_id: self.id(),
                kind: EventKind::WillCheckCancellation,
            });
            self.unwind_cancelled();
        }
    }

    #[cold]
    pub(crate) fn unwind_cancelled(&self) {
        self.report_untracked_read();
        Cancelled::PendingWrite.throw();
    }

    pub(crate) fn set_cancellation_flag(&self) {
        self.shared_state.revision_canceled.store(true);
    }

    /// Increments the "current revision" counter and clears
    /// the cancellation flag.
    ///
    /// This should only be done by the storage when the state is "quiescent".
    pub(crate) fn new_revision(&mut self) -> Revision {
        let r_old = self.current_revision();
        let r_new = r_old.next();
        self.shared_state.revisions[0].store(r_new);
        self.shared_state.revision_canceled.store(false);
        r_new
    }

    #[inline]
    pub(crate) fn push_query(&self, database_key_index: DatabaseKeyIndex) -> ActiveQueryGuard<'_> {
        self.local_state.push_query(database_key_index)
    }

    /// Block until `other_id` completes executing `database_key`;
    /// panic or unwind in the case of a cycle.
    ///
    /// `query_mutex_guard` is the guard for the current query's state;
    /// it will be dropped after we have successfully registered the
    /// dependency.
    ///
    /// # Propagating panics
    ///
    /// If the thread `other_id` panics, then our thread is considered
    /// cancelled, so this function will panic with a `Cancelled` value.
    ///
    /// # Cycle handling
    ///
    /// If the thread `other_id` already depends on the current thread,
    /// and hence there is a cycle in the query graph, then this function
    /// will unwind instead of returning normally. The method of unwinding
    /// depends on the [`Self::mutual_cycle_recovery_strategy`]
    /// of the cycle participants:
    ///
    /// * [`CycleRecoveryStrategy::Panic`]: panic with the [`Cycle`] as the value.
    /// * [`CycleRecoveryStrategy::Fallback`]: initiate unwinding with [`CycleParticipant::unwind`].
    pub(crate) fn block_on_or_unwind<QueryMutexGuard>(
        &self,
        db: &dyn Database,
        database_key: DatabaseKeyIndex,
        other_id: RuntimeId,
        query_mutex_guard: QueryMutexGuard,
    ) {
        let mut dg = self.shared_state.dependency_graph.lock();

        if dg.depends_on(other_id, self.id()) {
            self.unblock_cycle_and_maybe_throw(db, &mut dg, database_key, other_id);

            // If the above fn returns, then (via cycle recovery) it has unblocked the
            // cycle, so we can continue.
            assert!(!dg.depends_on(other_id, self.id()));
        }

        db.salsa_event(Event {
            runtime_id: self.id(),
            kind: EventKind::WillBlockOn {
                other_runtime_id: other_id,
                database_key,
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

        match result {
            WaitResult::Completed => (),

            // If the other thread panicked, then we consider this thread
            // cancelled. The assumption is that the panic will be detected
            // by the other thread and responded to appropriately.
            WaitResult::Panicked => Cancelled::PropagatedPanic.throw(),

            WaitResult::Cycle(c) => c.throw(),
        }
    }

    /// Handles a cycle in the dependency graph that was detected when the
    /// current thread tried to block on `database_key_index` which is being
    /// executed by `to_id`. If this function returns, then `to_id` no longer
    /// depends on the current thread, and so we should continue executing
    /// as normal. Otherwise, the function will throw a `Cycle` which is expected
    /// to be caught by some frame on our stack. This occurs either if there is
    /// a frame on our stack with cycle recovery (possibly the top one!) or if there
    /// is no cycle recovery at all.
    fn unblock_cycle_and_maybe_throw(
        &self,
        db: &dyn Database,
        dg: &mut DependencyGraph,
        database_key_index: DatabaseKeyIndex,
        to_id: RuntimeId,
    ) {
        log::debug!(
            "unblock_cycle_and_maybe_throw(database_key={:?})",
            database_key_index
        );

        let mut from_stack = self.local_state.take_query_stack();
        let from_id = self.id();

        // Make a "dummy stack frame". As we iterate through the cycle, we will collect the
        // inputs from each participant. Then, if we are participating in cycle recovery, we
        // will propagate those results to all participants.
        let mut cycle_query = ActiveQuery::new(database_key_index);

        // Identify the cycle participants:
        let cycle = {
            let mut v = vec![];
            dg.for_each_cycle_participant(
                from_id,
                &mut from_stack,
                database_key_index,
                to_id,
                |aqs| {
                    aqs.iter_mut().for_each(|aq| {
                        cycle_query.add_from(aq);
                        v.push(aq.database_key_index);
                    });
                },
            );

            // We want to give the participants in a deterministic order
            // (at least for this execution, not necessarily across executions),
            // no matter where it started on the stack. Find the minimum
            // key and rotate it to the front.
            let min = v.iter().min().unwrap();
            let index = v.iter().position(|p| p == min).unwrap();
            v.rotate_left(index);

            // No need to store extra memory.
            v.shrink_to_fit();

            Cycle::new(Arc::new(v))
        };
        log::debug!(
            "cycle {:?}, cycle_query {:#?}",
            cycle.debug(db),
            cycle_query,
        );

        // We can remove the cycle participants from the list of dependencies;
        // they are a strongly connected component (SCC) and we only care about
        // dependencies to things outside the SCC that control whether it will
        // form again.
        cycle_query.remove_cycle_participants(&cycle);

        // Mark each cycle participant that has recovery set, along with
        // any frames that come after them on the same thread. Those frames
        // are going to be unwound so that fallback can occur.
        dg.for_each_cycle_participant(from_id, &mut from_stack, database_key_index, to_id, |aqs| {
            aqs.iter_mut()
                .skip_while(|aq| {
                    match db.cycle_recovery_strategy(aq.database_key_index.ingredient_index) {
                        CycleRecoveryStrategy::Panic => true,
                        CycleRecoveryStrategy::Fallback => false,
                    }
                })
                .for_each(|aq| {
                    log::debug!("marking {:?} for fallback", aq.database_key_index.debug(db));
                    aq.take_inputs_from(&cycle_query);
                    assert!(aq.cycle.is_none());
                    aq.cycle = Some(cycle.clone());
                });
        });

        // Unblock every thread that has cycle recovery with a `WaitResult::Cycle`.
        // They will throw the cycle, which will be caught by the frame that has
        // cycle recovery so that it can execute that recovery.
        let (me_recovered, others_recovered) =
            dg.maybe_unblock_runtimes_in_cycle(from_id, &from_stack, database_key_index, to_id);

        self.local_state.restore_query_stack(from_stack);

        if me_recovered {
            // If the current thread has recovery, we want to throw
            // so that it can begin.
            cycle.throw()
        } else if others_recovered {
            // If other threads have recovery but we didn't: return and we will block on them.
        } else {
            // if nobody has recover, then we panic
            panic_any(cycle);
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
            .unblock_runtimes_blocked_on(database_key, wait_result);
    }
}
