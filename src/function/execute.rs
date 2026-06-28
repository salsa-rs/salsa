use smallvec::SmallVec;

use crate::active_query::CompletedQuery;
use crate::cycle::{CycleHeads, IterationStamp};
use crate::function::cycle_strategy::{CycleStrategy, ExecuteContext};
use crate::function::memo::{ErasedMemo, Memo, MemoHeader};
use crate::function::sync::ReleaseMode;
use crate::function::{ClaimGuard, ClaimResult, Configuration, IngredientImpl, Reentrancy};
use crate::hash::{FxHashSet, FxIndexSet};
use crate::plumbing::ZalsaLocal;
use crate::sync::thread;
use crate::tracked_struct::Identity;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{ActiveQueryGuard, QueryEdge, QueryEdgeKind, QueryRevisions};
use crate::{Cancelled, Cycle, tracing};
use crate::{DatabaseKeyIndex, Event, EventKind, Id};

impl<C: Configuration> IngredientImpl<C> {
    /// Executes this query through the shared query lifecycle and restores its typed memo.
    pub(super) fn execute<'db>(
        &'db self,
        db: &'db C::DbView,
        claim_guard: ClaimGuard<'db>,
        opt_old_memo: Option<&'db Memo<'db, C>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        report_will_execute(&claim_guard);

        <C::CycleStrategy as CycleStrategy<C>>::execute(ExecuteContext {
            ingredient: self,
            db,
            claim_guard,
            opt_old_memo,
            memo_ingredient_index,
        })
        .0
    }

    pub(super) fn execute_cycle<'db>(
        &'db self,
        db: &'db C::DbView,
        mut claim_guard: ClaimGuard<'db>,
        opt_old_memo: Option<&'db Memo<'db, C>>,
        memo_ingredient_index: MemoIngredientIndex,
        policy: CyclePolicy,
    ) -> Option<&'db Memo<'db, C>> {
        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();
        let id = database_key_index.key_index();

        let _cancellation_guard = DisableLocalCancellationGuard::new(claim_guard.zalsa_local());
        let _poison_guard = PoisonProvisionalIfPanicking {
            ingredient: self,
            zalsa,
            id,
            memo_ingredient_index,
        };
        let opt_old_memo_erased = opt_old_memo.map(Memo::erase);
        let mut state = CycleStateImpl::new(self, db);
        let completed_query = execute_maybe_iterate_erased(
            &mut state,
            zalsa,
            opt_old_memo_erased,
            &mut claim_guard,
            memo_ingredient_index,
            policy,
        );
        let value = state
            .value
            .take()
            .expect("query execution must produce a value");
        let memo = self.finish_memo(
            zalsa,
            database_key_index,
            opt_old_memo,
            value,
            completed_query,
            memo_ingredient_index,
        );

        if claim_guard.drop() { None } else { Some(memo) }
    }

    pub(super) fn execute_panic<'db>(
        &'db self,
        db: &'db C::DbView,
        claim_guard: ClaimGuard<'db>,
        opt_old_memo: Option<&'db Memo<'db, C>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();
        let id = database_key_index.key_index();

        let active_query = claim_guard.zalsa_local().push_query(database_key_index);
        if let Some(old_memo) = opt_old_memo {
            old_memo.header.seed_active_query(zalsa, &active_query);
        }
        let new_value = C::execute(db, C::id_to_input(zalsa, id));
        let completed_query = active_query.pop(IterationStamp::default());

        let memo = self.finish_memo(
            zalsa,
            database_key_index,
            opt_old_memo,
            new_value,
            completed_query,
            memo_ingredient_index,
        );

        if claim_guard.drop() { None } else { Some(memo) }
    }

    fn finish_memo<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        database_key_index: DatabaseKeyIndex,
        opt_old_memo: Option<&'db Memo<'db, C>>,
        value: C::Output<'db>,
        mut completed_query: CompletedQuery,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> &'db Memo<'db, C> {
        if let Some(old_memo) = opt_old_memo {
            // If the new value is equal to the old one, then it didn't
            // really change, even if some of its inputs have. So we can
            // "backdate" its `changed_at` revision to be the same as the
            // old value.
            self.backdate_if_appropriate(
                old_memo,
                database_key_index,
                &mut completed_query.revisions,
                &value,
            );

            // Diff the new outputs with the old, to discard any no-longer-emitted
            // outputs and update the tracked struct IDs for seeding the next revision.
            old_memo
                .header
                .diff_outputs(zalsa, database_key_index, &completed_query);
        }

        #[cfg(not(feature = "persistence"))]
        completed_query.revisions.discard_edges_if_never_change();

        self.insert_memo(
            zalsa,
            database_key_index.key_index(),
            Memo::new(
                Some(value),
                zalsa.current_revision(),
                completed_query.revisions,
            ),
            memo_ingredient_index,
        )
    }
}

fn report_will_execute(claim_guard: &ClaimGuard<'_>) {
    let database_key_index = claim_guard.database_key_index();

    crate::tracing::info!("{:?}: executing query", database_key_index);
    claim_guard.zalsa().event(&|| {
        Event::new(EventKind::WillExecute {
            database_key: database_key_index,
        })
    });
}

fn execute_maybe_iterate_erased<'db>(
    state: &mut dyn CycleState<'db>,
    zalsa: &'db Zalsa,
    opt_old_memo: Option<ErasedMemo<'db>>,
    claim_guard: &mut ClaimGuard<'db>,
    memo_ingredient_index: MemoIngredientIndex,
    policy: CyclePolicy,
) -> CompletedQuery {
    claim_guard.set_release_mode(ReleaseMode::Default);

    let database_key_index = claim_guard.database_key_index();

    let id = database_key_index.key_index();

    // Our provisional value from the previous iteration, when doing fixpoint iteration.
    // This is different from `opt_old_memo` which might be from a different revision.
    let mut last_provisional_memo_opt = None;

    let mut last_stale_tracked_ids: Vec<(Identity, Id)> = Vec::new();
    let cancellation_count = zalsa.runtime().cancellation_count();
    let mut iteration = IterationStamp::initial(cancellation_count);

    if let Some(old_memo) = opt_old_memo {
        if let Some(previous_iteration) = old_memo.header().previous_iteration(
            zalsa,
            database_key_index,
            cancellation_count,
            old_memo.has_value(),
        ) {
            if previous_iteration.reuse_as_provisional {
                last_provisional_memo_opt = Some(old_memo);
            }

            iteration = previous_iteration.iteration;
        }
    }

    let completed_query = loop {
        let active_query = claim_guard.zalsa_local().push_query(database_key_index);

        // Tracked struct ids that existed in the previous revision
        // but weren't recreated in the last iteration. It's important that we seed the next
        // query with these ids because the query might re-create them as part of the next iteration.
        // This is not only important to ensure that the re-created tracked structs have the same ids,
        // it's also important to ensure that these tracked structs get removed
        // if they aren't recreated when reaching the final iteration.
        active_query.seed_tracked_struct_ids(&last_stale_tracked_ids);

        seed_query_from_old_memo(
            zalsa,
            &active_query,
            last_provisional_memo_opt.or(opt_old_memo),
        );

        state.execute_query(zalsa, id);
        let (mut active_query, cycle_heads, outer_cycle, cycle_iteration) =
            match try_complete_query(zalsa, active_query, claim_guard, iteration) {
                QueryExecutionOutcome::Completed(completed_query) => break completed_query,
                QueryExecutionOutcome::Participant {
                    active_query,
                    cycle_heads,
                    outer_cycle,
                } => {
                    policy.complete_participant(state, zalsa, id);

                    break complete_cycle_participant(
                        active_query,
                        claim_guard,
                        cycle_heads,
                        outer_cycle,
                        iteration,
                    );
                }
                QueryExecutionOutcome::CycleHead {
                    active_query,
                    cycle_heads,
                    outer_cycle,
                    cycle_iteration,
                } => (active_query, cycle_heads, outer_cycle, cycle_iteration),
            };

        // Get the last provisional value for this query so that we can compare it with the new value
        // to test if the cycle converged.
        let last_provisional_memo = last_provisional_memo_opt.unwrap_or_else(|| {
            // This is our first time around the loop; a provisional value must have been
            // inserted into the memo table when the cycle was hit, so let's pull our
            // initial provisional value from there.
            let memo = state
                .provisional_memo(zalsa, id, memo_ingredient_index)
                .unwrap_or_else(|| {
                    unreachable!(
                        "{database_key_index:#?} is a cycle head, \
                                        but no provisional memo found"
                    )
                });

            debug_assert!(
                !memo
                    .header()
                    .revisions
                    .verified_final
                    .load(std::sync::atomic::Ordering::Relaxed)
            );
            memo
        });
        crate::tracing::debug!(
            "{database_key_index:?}: execute: \
                I am a cycle head, comparing last provisional value with new value"
        );

        let cycle = Cycle {
            head_ids: cycle_heads.ids(),
            id,
            iteration: cycle_iteration.iteration_as_u32(),
        };
        let value_converged =
            policy.recover_cycle_head(state, zalsa, id, &cycle, last_provisional_memo);

        let new_cycle_heads = active_query.take_cycle_heads();
        assert_no_new_cycle_heads(&cycle_heads, new_cycle_heads, database_key_index);

        let completed_query = match try_complete_cycle_head(
            active_query,
            claim_guard,
            cycle_heads,
            &last_provisional_memo.header().revisions,
            outer_cycle,
            iteration,
            cycle_iteration,
            value_converged,
        ) {
            Ok(completed_query) => {
                break completed_query;
            }
            Err((completed_query, new_iteration)) => {
                iteration = new_iteration;
                completed_query
            }
        };

        let new_memo = state.insert_provisional_memo(
            zalsa,
            id,
            completed_query.revisions,
            memo_ingredient_index,
        );

        last_provisional_memo_opt = Some(new_memo);

        last_stale_tracked_ids = completed_query.stale_tracked_structs;
    };

    crate::tracing::debug!(
        "{database_key_index:?}: execute_maybe_iterate: result.revisions = {revisions:#?}",
        revisions = &completed_query.revisions
    );

    completed_query
}

#[derive(Copy, Clone)]
pub(super) enum CyclePolicy {
    FallbackImmediate,
    Fixpoint,
}

impl CyclePolicy {
    /// Adjusts the query value before completing a non-head cycle participant.
    ///
    /// Fallback recovery replaces the computed value with the fallback. Fixpoint recovery keeps
    /// the computed value unchanged.
    fn complete_participant<'db>(self, state: &mut dyn CycleState<'db>, zalsa: &'db Zalsa, id: Id) {
        if matches!(self, Self::FallbackImmediate) {
            state.use_fallback(zalsa, id);
        }
    }

    /// Recovers the value produced by the latest cycle-head iteration.
    ///
    /// Returns whether the query value converged. A `true` result does not mean the entire cycle
    /// converged: cycle-head metadata is compared separately by `try_complete_cycle_head`.
    fn recover_cycle_head<'db>(
        self,
        state: &mut dyn CycleState<'db>,
        zalsa: &'db Zalsa,
        id: Id,
        cycle: &Cycle,
        last_provisional_memo: ErasedMemo<'db>,
    ) -> bool {
        match self {
            Self::FallbackImmediate => {
                state.use_fallback(zalsa, id);
                true
            }
            Self::Fixpoint => state.recover_from_cycle(zalsa, cycle, last_provisional_memo),
        }
    }
}

/// Type-specific operations needed by recoverable cycle handling.
///
/// This erased bridge is used only by fallback and fixpoint cycle strategies. Every
/// [`ErasedMemo`] passed to a state must come from that state's ingredient.
pub(super) trait CycleState<'db> {
    fn execute_query(&mut self, zalsa: &'db Zalsa, id: Id);

    fn use_fallback(&mut self, zalsa: &'db Zalsa, id: Id);

    fn recover_from_cycle(
        &mut self,
        zalsa: &'db Zalsa,
        cycle: &Cycle,
        last_provisional_memo: ErasedMemo<'db>,
    ) -> bool;

    fn provisional_memo(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ErasedMemo<'db>>;

    fn insert_provisional_memo(
        &mut self,
        zalsa: &'db Zalsa,
        id: Id,
        revisions: QueryRevisions,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> ErasedMemo<'db>;
}

pub(super) struct CycleStateImpl<'db, C: Configuration> {
    ingredient: &'db IngredientImpl<C>,
    db: &'db C::DbView,
    value: Option<C::Output<'db>>,
}

impl<'db, C: Configuration> CycleStateImpl<'db, C> {
    pub(super) fn new(ingredient: &'db IngredientImpl<C>, db: &'db C::DbView) -> Self {
        Self {
            ingredient,
            db,
            value: None,
        }
    }
}

impl<'db, C: Configuration> CycleState<'db> for CycleStateImpl<'db, C> {
    fn execute_query(&mut self, zalsa: &'db Zalsa, id: Id) {
        self.value = Some(C::execute(self.db, C::id_to_input(zalsa, id)));
    }

    fn use_fallback(&mut self, zalsa: &'db Zalsa, id: Id) {
        self.value = Some(C::cycle_initial(self.db, id, C::id_to_input(zalsa, id)));
    }

    fn recover_from_cycle(
        &mut self,
        zalsa: &'db Zalsa,
        cycle: &Cycle,
        last_provisional_memo: ErasedMemo<'db>,
    ) -> bool {
        let last_provisional_memo = last_provisional_memo.downcast::<C>();
        let last_provisional_value = last_provisional_memo.value.as_ref().expect(
            "`fetch_cold_cycle` should have inserted a provisional memo with Cycle::initial",
        );
        let value = self
            .value
            .take()
            .expect("cycle state must contain the value from the latest execution");
        let value = C::recover_from_cycle(
            self.db,
            cycle,
            last_provisional_value,
            value,
            C::id_to_input(zalsa, cycle.id),
        );
        let converged = C::values_equal(&value, last_provisional_value);
        self.value = Some(value);
        converged
    }

    fn provisional_memo(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<ErasedMemo<'db>> {
        self.ingredient
            .memo_table_for(zalsa, id)
            .get_erased(memo_ingredient_index)
    }

    fn insert_provisional_memo(
        &mut self,
        zalsa: &'db Zalsa,
        id: Id,
        revisions: QueryRevisions,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> ErasedMemo<'db> {
        let value = self
            .value
            .take()
            .expect("cycle state must contain a value before memo insertion");
        self.ingredient
            .insert_memo(
                zalsa,
                id,
                Memo::new(Some(value), zalsa.current_revision(), revisions),
                memo_ingredient_index,
            )
            .erase()
    }
}

fn seed_query_from_old_memo(
    zalsa: &Zalsa,
    active_query: &ActiveQueryGuard<'_>,
    old_memo: Option<ErasedMemo<'_>>,
) {
    let Some(old_memo) = old_memo else {
        return;
    };

    old_memo.header().seed_active_query(zalsa, active_query);
}

struct PreviousIteration {
    iteration: IterationStamp,
    reuse_as_provisional: bool,
}

enum QueryExecutionOutcome<'db> {
    Completed(CompletedQuery),
    Participant {
        active_query: ActiveQueryGuard<'db>,
        cycle_heads: CycleHeads,
        outer_cycle: DatabaseKeyIndex,
    },
    CycleHead {
        active_query: ActiveQueryGuard<'db>,
        cycle_heads: CycleHeads,
        outer_cycle: Option<DatabaseKeyIndex>,
        cycle_iteration: IterationStamp,
    },
}

impl MemoHeader {
    fn previous_iteration(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        cancellation_count: u8,
        has_value: bool,
    ) -> Option<PreviousIteration> {
        if self.verified_at.load() != zalsa.current_revision()
            || self.revisions.iteration().cancellation_count() != cancellation_count
        {
            return None;
        }

        // The `DependencyGraph` locking propagates panics when another thread is blocked on a panicking query.
        // However, the locking doesn't handle the case where a thread fetches the result of a panicking
        // cycle head query **after** all locks were released. That's what we do here.
        // We could consider re-executing the entire cycle but:
        // a) It's tricky to ensure that all queries participating in the cycle will re-execute
        //    (we can't rely on `iteration` being updated for nested cycles because the nested cycles may have completed successfully).
        // b) It's guaranteed that this query will panic again anyway.
        // That's why we simply propagate the panic here. It simplifies our lives and it also avoids duplicate panic messages.
        if !has_value {
            tracing::warn!(
                "Propagating panic for cycle head that panicked in an earlier execution in that revision"
            );
            Cancelled::PropagatedPanic.throw();
        }

        Some(PreviousIteration {
            iteration: self.revisions.iteration(),
            // Only use the last provisional memo if it was a cycle head in the last iteration. This is to
            // force at least two executions.
            reuse_as_provisional: self.cycle_heads().contains(&database_key_index),
        })
    }

    fn seed_active_query(&self, zalsa: &Zalsa, active_query: &ActiveQueryGuard<'_>) {
        // If we already executed this query once, then use the tracked-struct ids from the
        // previous execution as the starting point for the new one.
        active_query.seed_tracked_struct_ids(self.revisions.tracked_struct_ids());

        // Copy over all inputs and outputs from a previous iteration.
        // This is necessary to:
        // * ensure that tracked struct created during the previous iteration
        //   (and are owned by the query) are alive even if the query in this iteration no longer creates them.
        // * ensure the final returned memo depends on all inputs from all iterations.
        if self.may_be_provisional() && self.verified_at.load() == zalsa.current_revision() {
            active_query.seed_iteration(&self.revisions);
        }
    }
}

fn try_complete_query<'db>(
    zalsa: &Zalsa,
    mut active_query: ActiveQueryGuard<'db>,
    claim_guard: &mut ClaimGuard<'db>,
    iteration: IterationStamp,
) -> QueryExecutionOutcome<'db> {
    let database_key_index = active_query.database_key_index;

    // Take the cycle heads to not-fight-rust's-borrow-checker.
    let mut cycle_heads = active_query.take_cycle_heads();

    // If there are no cycle heads, break out of the loop.
    if cycle_heads.is_empty() {
        // There's no cycle iteration state to preserve.
        let iteration = if iteration.is_initial_iteration() {
            IterationStamp::default()
        } else {
            iteration.increment_iteration().unwrap_or_else(|| {
                tracing::warn!("{database_key_index:?}: execute: too many cycle iterations");
                panic!("{database_key_index:?}: execute: too many cycle iterations")
            })
        };

        return QueryExecutionOutcome::Completed(active_query.pop(iteration));
    }

    let (max_iteration, depends_on_self) =
        collect_all_cycle_heads(zalsa, &mut cycle_heads, database_key_index, iteration);

    let outer_cycle = outer_cycle(
        zalsa,
        claim_guard.zalsa_local(),
        &cycle_heads,
        database_key_index,
    );

    // Did the new result we got depend on our own provisional value, in a cycle?
    // If not, return because this query is not a cycle head.
    if !depends_on_self {
        let Some(outer_cycle) = outer_cycle else {
            panic!(
                "cycle participant with non-empty cycle heads and that doesn't depend on itself must have an outer cycle responsible to finalize the query later (query: {database_key_index:?}, cycle heads: {cycle_heads:?})."
            );
        };

        return QueryExecutionOutcome::Participant {
            active_query,
            cycle_heads,
            outer_cycle,
        };
    }

    // If this is the outermost cycle, use the maximum iteration count of all cycles.
    // This is important for when later iterations introduce new cycle heads (that then
    // become the outermost cycle). We want to ensure that the iteration count keeps increasing
    // for all queries or they won't be re-executed because `validate_same_iteration` would
    // pass when we go from 1 -> 0 and then increment by 1 to 1).
    let cycle_iteration = if outer_cycle.is_none() {
        max_iteration
    } else {
        // Otherwise keep the iteration count because outer cycles
        // already have a cycle head with this exact iteration count (and we don't allow
        // heads from different iterations).
        iteration
    };

    QueryExecutionOutcome::CycleHead {
        active_query,
        cycle_heads,
        outer_cycle,
        cycle_iteration,
    }
}

#[must_use]
struct DisableLocalCancellationGuard<'a> {
    zalsa_local: &'a ZalsaLocal,
    was_disabled: bool,
}

impl<'a> DisableLocalCancellationGuard<'a> {
    fn new(zalsa_local: &'a ZalsaLocal) -> Self {
        Self {
            zalsa_local,
            was_disabled: zalsa_local.set_cancellation_disabled(true),
        }
    }
}

impl Drop for DisableLocalCancellationGuard<'_> {
    fn drop(&mut self) {
        self.zalsa_local
            .set_cancellation_disabled(self.was_disabled);
    }
}

/// Replaces any inserted memo with a fixpoint initial memo without a value if the current thread panics.
///
/// A regular query doesn't insert any memo if it panics and the query
/// simply gets re-executed if any later called query depends on the panicked query (and will panic again unless the query isn't deterministic).
///
/// Unfortunately, this isn't the case for cycle heads because Salsa first inserts the fixpoint initial memo and later inserts
/// provisional memos for every iteration. Detecting whether a query has previously panicked
/// in `fetch` (e.g., `validate_same_iteration`) and requires re-execution is probably possible but not very straightforward
/// and it's easy to get it wrong, which results in infinite loops where `Memo::provisional_retry` keeps retrying to get the latest `Memo`
/// but `fetch` doesn't re-execute the query for reasons.
///
/// Specifically, a Memo can linger after a panic, which is then incorrectly returned
/// by `fetch_cold_cycle` because it passes the `shallow_verified_memo` check instead of inserting
/// a new fix point initial value if that happens.
///
/// We could insert a fixpoint initial value here, but it seems unnecessary.
struct PoisonProvisionalIfPanicking<'a, C: Configuration> {
    ingredient: &'a IngredientImpl<C>,
    zalsa: &'a Zalsa,
    id: Id,
    memo_ingredient_index: MemoIngredientIndex,
}

impl<C: Configuration> Drop for PoisonProvisionalIfPanicking<'_, C> {
    fn drop(&mut self) {
        if thread::panicking() {
            let revisions = QueryRevisions::fixpoint_initial(
                self.ingredient.database_key_index(self.id),
                IterationStamp::initial(self.zalsa.runtime().cancellation_count()),
            );

            let memo = Memo::new(None, self.zalsa.current_revision(), revisions);
            self.ingredient
                .insert_memo(self.zalsa, self.id, memo, self.memo_ingredient_index);
        }
    }
}

/// Returns the key of any potential outer cycle head or `None` if there is no outer cycle.
///
/// That is, any query that's currently blocked on the result computed by this query (claiming it results in a cycle).
fn outer_cycle(
    zalsa: &Zalsa,
    zalsa_local: &ZalsaLocal,
    cycle_heads: &CycleHeads,
    current_key: DatabaseKeyIndex,
) -> Option<DatabaseKeyIndex> {
    // First, look for the outer most cycle head on the same thread.
    // Using the outer most over the inner most should reduce the need
    // for transitive transfers.
    // SAFETY: We don't call into with_query_stack recursively
    if let Some(same_thread) = unsafe {
        zalsa_local.with_query_stack_unchecked(|stack| {
            stack
                .iter()
                .find(|active_query| {
                    active_query.database_key_index != current_key
                        && cycle_heads.contains(&active_query.database_key_index)
                })
                .map(|active_query| active_query.database_key_index)
        })
    } {
        return Some(same_thread);
    }

    // Check for any outer cycle head running on a different thread.
    cycle_heads
        .iter_not_eq(current_key)
        .rfind(|head| {
            let function = zalsa
                .lookup_ingredient(head.database_key_index.ingredient_index())
                .as_function()
                .expect("cycle heads must be function ingredients");

            matches!(
                function.sync_table().peek_claim(
                    zalsa,
                    head.database_key_index.key_index(),
                    Reentrancy::Deny,
                ),
                ClaimResult::Cycle { inner: false }
            )
        })
        .map(|head| head.database_key_index)
}

/// Ensure that we resolve the latest cycle heads from any provisional value this query depended on during execution.
///
/// ```txt
/// E -> C -> D -> B -> A -> B (cycle)
///                     -- A completes, heads = [B]
/// E -> C -> D -> B -> C (cycle)
///                  -> D (cycle)
///                -- B completes, heads = [B, C, D]
/// E -> C -> D -> E (cycle)
///           -- D completes, heads = [E, B, C, D]
/// E -> C
///      -- C completes, heads = [E, B, C, D]
/// E -> X -> A
///      -- X completes, heads = [B]
/// ```
///
/// Note how `X` only depends on `A`. It doesn't know that it's part of the outer cycle `X`.
/// An old implementation resolved the cycle heads 1-level deep but that's not enough, because
/// `X` then completes with `[B, C, D]` as it's heads. But `B`, `C`, and `D` are no longer on the stack
/// when `X` completes (which is the real outermost cycle). That's why we need to resolve all cycle heads
/// recursively, so that `X` completes with `[B, C, D, E]
fn collect_all_cycle_heads(
    zalsa: &Zalsa,
    cycle_heads: &mut CycleHeads,
    database_key_index: DatabaseKeyIndex,
    iteration: IterationStamp,
) -> (IterationStamp, bool) {
    fn collect_recursive(
        zalsa: &Zalsa,
        current_head: DatabaseKeyIndex,
        me: DatabaseKeyIndex,
        query_heads: &CycleHeads,
        missing_heads: &mut SmallVec<[(DatabaseKeyIndex, IterationStamp); 4]>,
    ) -> (IterationStamp, bool) {
        if current_head == me {
            return (IterationStamp::default(), true);
        }

        let mut max_iteration = IterationStamp::default();
        let mut depends_on_self = false;

        let function = zalsa
            .lookup_ingredient(current_head.ingredient_index())
            .as_function()
            .expect("cycle heads must be function ingredients");

        let provisional_status = function
            .memo(zalsa, current_head.key_index())
            .map(|memo| memo.header().provisional_status())
            .expect("cycle head memo must have been created during the execution");

        // A query should only ever depend on other heads that are provisional.
        // If this invariant is violated, it means that this query participates in a cycle,
        // but it wasn't executed in the last iteration of said cycle.
        assert!(provisional_status.is_provisional());

        for head in provisional_status.cycle_heads() {
            let iteration = head.iteration.load();
            max_iteration = max_iteration.max(iteration);

            if query_heads.contains(&head.database_key_index) {
                continue;
            }

            let head_as_tuple = (head.database_key_index, iteration);

            if missing_heads.contains(&head_as_tuple) {
                continue;
            }

            missing_heads.push((head.database_key_index, iteration));

            let (nested_max_iteration, nested_depends_on_self) = collect_recursive(
                zalsa,
                head.database_key_index,
                me,
                query_heads,
                missing_heads,
            );

            max_iteration = max_iteration.max(nested_max_iteration);
            depends_on_self |= nested_depends_on_self;
        }

        (max_iteration, depends_on_self)
    }

    let mut missing_heads: SmallVec<[(DatabaseKeyIndex, IterationStamp); 4]> = SmallVec::new();
    let mut max_iteration = iteration;
    let mut depends_on_self = false;
    for head in &*cycle_heads {
        let (recursive_max_iteration, recursive_depends_on_self) = collect_recursive(
            zalsa,
            head.database_key_index,
            database_key_index,
            cycle_heads,
            &mut missing_heads,
        );

        max_iteration = max_iteration.max(recursive_max_iteration);
        depends_on_self |= recursive_depends_on_self;
    }

    for (head, iteration) in missing_heads {
        cycle_heads.insert(head, iteration);
    }

    (max_iteration, depends_on_self)
}

// Called when completing the query of a cycle head participating
// in an outer cycle head (which doesn't depend on itself).
fn complete_cycle_participant(
    active_query: ActiveQueryGuard,
    claim_guard: &mut ClaimGuard,
    cycle_heads: CycleHeads,
    outer_cycle: DatabaseKeyIndex,
    iteration: IterationStamp,
) -> CompletedQuery {
    // For as long as this query participates in any cycle, don't release its lock, instead
    // transfer it to the outermost cycle head. This prevents any other thread
    // from claiming this query (all cycle heads are potential entry points to the same cycle),
    // which would result in them competing for the same locks (we want the locks to converge to a single cycle head).
    claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));
    let zalsa = claim_guard.zalsa();

    let database_key_index = active_query.database_key_index;
    let iteration = iteration.increment_iteration().unwrap_or_else(|| {
        tracing::warn!("{database_key_index:?}: execute: too many cycle iterations");
        panic!("{database_key_index:?}: execute: too many cycle iterations")
    });

    let mut completed_query = complete_cycle_query(zalsa, active_query, iteration);

    *completed_query.revisions.verified_final.get_mut() = false;
    completed_query
        .revisions
        .set_cycle_heads(cycle_heads, iteration);

    completed_query
}

/// Tries to complete the cycle head if it has converged.
///
/// Returns `Ok` if the cycle head has converged or if it is part of an outer cycle.
/// Returns `Err` if the cycle head needs to keep iterating.
#[allow(clippy::too_many_arguments)]
fn try_complete_cycle_head(
    active_query: ActiveQueryGuard,
    claim_guard: &mut ClaimGuard,
    mut cycle_heads: CycleHeads,
    last_provisional_revisions: &QueryRevisions,
    outer_cycle: Option<DatabaseKeyIndex>,
    iteration: IterationStamp,
    max_iteration: IterationStamp,
    value_converged: bool,
) -> Result<CompletedQuery, (CompletedQuery, IterationStamp)> {
    let me = active_query.database_key_index;
    let zalsa = claim_guard.zalsa();

    let mut completed_query = complete_cycle_query(zalsa, active_query, iteration);

    // It's important to force a re-execution of the cycle if `changed_at` or `durability` has changed
    // to ensure the reduced durability and changed propagates to all queries depending on this head.
    let metadata_converged = last_provisional_revisions.durability
        == completed_query.revisions.durability
        && last_provisional_revisions.changed_at == completed_query.revisions.changed_at
        && last_provisional_revisions.is_derived_untracked()
            == completed_query.revisions.is_derived_untracked();

    let this_converged = value_converged && metadata_converged;

    if let Some(outer_cycle) = outer_cycle {
        tracing::info!(
            "Detected nested cycle {me:?}, iterate it as part of the outer cycle {outer_cycle:?}"
        );

        completed_query
            .revisions
            .set_cycle_heads(cycle_heads, max_iteration);
        // Store whether this cycle has converged, so that the outer cycle can check it.
        completed_query
            .revisions
            .set_cycle_converged(this_converged);
        *completed_query.revisions.verified_final.get_mut() = false;

        // Transfer ownership of this query to the outer cycle, so that it can claim it
        // and other threads don't compete for the same lock.
        claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));

        return Ok(completed_query);
    }

    // This is the outermost cycle, drive the cycle forward:
    // ..test if all inner cycles have converged as well.
    let converged = this_converged
        && cycle_heads.iter_not_eq(me).all(|head| {
            let database_key_index = head.database_key_index;
            let function = zalsa
                .lookup_ingredient(database_key_index.ingredient_index())
                .as_function()
                .expect("cycle heads must be function ingredients");

            let converged = function
                .memo(zalsa, database_key_index.key_index())
                .is_none_or(|memo| memo.header().cycle_converged());

            if !converged {
                tracing::debug!("inner cycle {database_key_index:?} has not converged",);
            }

            converged
        });

    if converged {
        tracing::debug!(
            "{me:?}: execute: fixpoint iteration has a final value after {max_iteration:?} iterations",
            max_iteration = max_iteration.iteration()
        );

        // Set the nested cycles as verified. This is necessary because
        // `validate_provisional` doesn't follow cycle heads recursively (and the memos now depend on all cycle heads).
        for head in cycle_heads.iter_not_eq(me) {
            let function = zalsa
                .lookup_ingredient(head.database_key_index.ingredient_index())
                .as_function()
                .expect("cycle heads must be function ingredients");
            if let Some(memo) = function.memo(zalsa, head.database_key_index.key_index()) {
                memo.header().finalize_cycle_head();
            }
        }

        *completed_query.revisions.verified_final.get_mut() = true;

        zalsa.event(&|| {
            Event::new(EventKind::DidFinalizeCycle {
                database_key: me,
                iteration: max_iteration.iteration(),
            })
        });

        return Ok(completed_query);
    }

    // The fixpoint iteration hasn't converged. Iterate again...
    let iteration = max_iteration.increment_iteration().unwrap_or_else(|| {
        tracing::warn!("{me:?}: execute: too many cycle iterations");
        panic!("{me:?}: execute: too many cycle iterations")
    });

    zalsa.event(&|| {
        Event::new(EventKind::WillIterateCycle {
            database_key: me,
            iteration: iteration.iteration(),
        })
    });

    tracing::info!(
        "{me:?}: execute: iterate again ({iteration:?})...",
        iteration = iteration.iteration()
    );

    // Update the iteration count of nested cycles.
    for head in cycle_heads.iter_not_eq(me) {
        let function = zalsa
            .lookup_ingredient(head.database_key_index.ingredient_index())
            .as_function()
            .expect("cycle heads must be function ingredients");
        if let Some(memo) = function.memo(zalsa, head.database_key_index.key_index()) {
            memo.header()
                .set_cycle_iteration_count(head.database_key_index, iteration);
        }
    }

    debug_assert!(completed_query.revisions.cycle_heads().is_empty());

    cycle_heads.update_iteration_count_mut(me, iteration);
    completed_query
        .revisions
        .set_cycle_heads(cycle_heads, iteration);
    *completed_query.revisions.verified_final.get_mut() = false;

    Err((completed_query, iteration))
}

fn assert_no_new_cycle_heads(
    cycle_heads: &CycleHeads,
    new_cycle_heads: CycleHeads,
    me: DatabaseKeyIndex,
) {
    for head in new_cycle_heads {
        if !cycle_heads.contains(&head.database_key_index) {
            panic!(
                "Cycle recovery function for {me:?} introduced a cycle, depending on {:?}. This is not allowed.",
                head.database_key_index
            );
        }
    }
}

thread_local! {
    /// Pool the `seen` and `flattened` sets for reuse on the same thread.
    ///
    /// Benchmarks showed that repeatedly allocating and regrowing those sets is expensive.
    static FLATTEN_MAPS: std::cell::Cell<Option<(FxIndexSet<QueryEdge>, FxHashSet<DatabaseKeyIndex>)>> = const { std::cell::Cell::new(None) };
}

fn complete_cycle_query(
    zalsa: &Zalsa,
    active_query: ActiveQueryGuard<'_>,
    iteration: IterationStamp,
) -> CompletedQuery {
    let (mut flattened, mut seen) = FLATTEN_MAPS.take().unwrap_or_default();

    debug_assert!(flattened.is_empty());
    debug_assert!(seen.is_empty());

    let detached_query = active_query.detach();
    flattened.reserve(detached_query.input_outputs().len());
    flatten_cycle_dependencies(
        zalsa,
        detached_query.input_outputs(),
        &mut flattened,
        &mut seen,
    );

    seen.clear();
    let completion = detached_query.pop_completion(iteration, true);
    let completed_query = completion.finish(flattened.drain(..));
    #[cfg(feature = "accumulator")]
    assert!(
        completed_query
            .revisions
            .accumulated_inputs
            .load()
            .is_empty(),
        "Fixpoint iteration doesn't support accumulated values because it doesn't preserve the original query dependency tree."
    );
    FLATTEN_MAPS.set(Some((flattened, seen)));
    completed_query
}

/// Flattens the dependencies of `head` so that `head`'s origin only depends on finalized queries,
/// or salsa structs (input, tracked, interned).
fn flatten_cycle_dependencies(
    zalsa: &Zalsa,
    direct_input_outputs: &FxIndexSet<QueryEdge>,
    flattened: &mut FxIndexSet<QueryEdge>,
    seen: &mut FxHashSet<DatabaseKeyIndex>,
) {
    // Don't insert the key of `head` here. This is important to ensure that we copy over the
    // dependencies from this memo in the previous iteration.
    // e.g. if we have `a2 -> b2 -> a1`, we need to copy over `a`'s dependencies from iteration 1.
    for edge in direct_input_outputs.iter().copied() {
        match edge.kind() {
            QueryEdgeKind::Input => {
                let input = edge.key();
                let ingredient = zalsa.lookup_ingredient(input.ingredient_index());
                ingredient.flatten_cycle_head_dependencies(
                    zalsa,
                    input.key_index(),
                    flattened,
                    seen,
                );
            }

            QueryEdgeKind::Output => {
                // Unlike `ingredient.collect_flattened_cycle_inputs`, carry over outputs
                // created by the query head because those are owned by this query.
                flattened.insert(edge);
            }
        }
    }
}
