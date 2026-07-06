use smallvec::SmallVec;

use crate::active_query::CompletedQuery;
use crate::cycle::{CycleHeads, CycleRecoveryStrategy, IterationStamp};
use crate::function::memo::{Memo, MemoHeader};
use crate::function::sync::ReleaseMode;
use crate::function::{ClaimGuard, Configuration, IngredientImpl};
use crate::hash::{FxHashSet, FxIndexSet, should_discard_retained_capacity};
use crate::ingredient::WaitForResult;
use crate::plumbing::ZalsaLocal;
use crate::sync::thread;
use crate::tracked_struct::Identity;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{ActiveQueryGuard, QueryEdge, QueryEdgeKind, QueryRevisions};
use crate::{Cancelled, Cycle, tracing};
use crate::{DatabaseKeyIndex, Event, EventKind, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Executes the query function for the given `active_query`. Creates and stores
    /// a new memo with the result, backdated if possible. Once this completes,
    /// the query will have been popped off the active query stack.
    ///
    /// # Parameters
    ///
    /// * `db`, the database.
    /// * `active_query`, the active stack frame for the query to execute.
    /// * `opt_old_memo`, the older memo, if any existed. Used for backdating.
    ///
    /// # Returns
    /// The newly computed memo or `None` if this query is part of a larger cycle
    /// and `execute` blocked on a cycle head running on another thread. In this case,
    /// the memo is potentially outdated and needs to be refetched.
    #[inline(never)]
    pub(super) fn execute<'db>(
        &'db self,
        db: &'db C::DbView,
        mut claim_guard: ClaimGuard<'db>,
        opt_old_memo: Option<&'db Memo<C>>,
    ) -> Option<&'db Memo<C>> {
        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();

        let id = database_key_index.key_index();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

        crate::tracing::info!("{:?}: executing query", database_key_index);

        zalsa.event(&|| {
            Event::new(EventKind::WillExecute {
                database_key: database_key_index,
            })
        });

        let (new_value, mut completed_query) = match C::CYCLE_STRATEGY {
            CycleRecoveryStrategy::Panic => {
                let (new_value, active_query) = Self::execute_query(
                    db,
                    zalsa,
                    claim_guard.zalsa_local().push_query(database_key_index),
                    opt_old_memo.map(|memo| &memo.header),
                );

                // Ordinary queries don't need a cycle iteration stamp. Keeping the default avoids
                // allocating `QueryRevisionsExtra` after a revision-preserving cancellation.
                (new_value, active_query.pop(IterationStamp::default()))
            }
            CycleRecoveryStrategy::FallbackImmediate | CycleRecoveryStrategy::Fixpoint => {
                let _cancellation_guard =
                    DisableLocalCancellationGuard::new(claim_guard.zalsa_local());

                self.execute_maybe_iterate(
                    db,
                    opt_old_memo,
                    &mut claim_guard,
                    memo_ingredient_index,
                )
            }
        };

        if let Some(old_memo) = opt_old_memo {
            // If the new value is equal to the old one, then it didn't
            // really change, even if some of its inputs have. So we can
            // "backdate" its `changed_at` revision to be the same as the
            // old value.
            self.backdate_if_appropriate(
                old_memo,
                database_key_index,
                &mut completed_query.revisions,
                &new_value,
            );

            // Diff the new outputs with the old, to discard any no-longer-emitted
            // outputs and update the tracked struct IDs for seeding the next revision.
            old_memo
                .header
                .diff_outputs(zalsa, database_key_index, &completed_query);
        }

        #[cfg(not(feature = "persistence"))]
        completed_query.revisions.discard_edges_if_never_change();

        let memo = self.insert_memo(
            zalsa,
            id,
            Memo::new(
                Some(new_value),
                zalsa.current_revision(),
                completed_query.revisions,
            ),
            memo_ingredient_index,
        );

        if claim_guard.drop() { None } else { Some(memo) }
    }

    fn execute_maybe_iterate<'db>(
        &'db self,
        db: &'db C::DbView,
        opt_old_memo: Option<&'db Memo<C>>,
        claim_guard: &mut ClaimGuard<'db>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> (C::Output<'db>, CompletedQuery) {
        claim_guard.set_release_mode(ReleaseMode::Default);

        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();

        let id = database_key_index.key_index();

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // This is different from `opt_old_memo` which might be from a different revision.
        let mut last_provisional_memo_opt: Option<&Memo<C>> = None;

        let mut last_stale_tracked_ids: Vec<(Identity, Id)> = Vec::new();
        let current_revision = zalsa.current_revision();
        let cancellation_count = zalsa.runtime().cancellation_count();
        let mut opt_old_memo = opt_old_memo;
        let mut iteration = IterationStamp::initial(cancellation_count);

        // An ordinary query doesn't memoize a cancelled execution. Match that behavior for
        // fixpoint queries: a memo from an abandoned cancellation epoch in this revision doesn't
        // seed the retry, while a memo from an older revision remains useful for backdating and
        // output bookkeeping. Cancellation counts are only comparable within a revision.
        if let Some(old_memo) = opt_old_memo {
            if old_memo.header.verified_at.load() == current_revision {
                match old_memo.header.previous_iteration(
                    database_key_index,
                    cancellation_count,
                    old_memo.value.is_some(),
                ) {
                    Some(previous_iteration) => {
                        if previous_iteration.reuse_as_provisional {
                            last_provisional_memo_opt = Some(old_memo);
                        }

                        iteration = previous_iteration.iteration;
                    }
                    None => opt_old_memo = None,
                }
            }
        }

        let _poison_guard =
            PoisonProvisionalIfPanicking::new(self, zalsa, id, memo_ingredient_index);

        let (new_value, completed_query) = loop {
            let active_query = claim_guard.zalsa_local().push_query(database_key_index);

            // Tracked struct ids that existed in the previous revision
            // but weren't recreated in the last iteration. It's important that we seed the next
            // query with these ids because the query might re-create them as part of the next iteration.
            // This is not only important to ensure that the re-created tracked structs have the same ids,
            // it's also important to ensure that these tracked structs get removed
            // if they aren't recreated when reaching the final iteration.
            active_query.seed_tracked_struct_ids(&last_stale_tracked_ids);

            let (mut new_value, active_query) = Self::execute_query(
                db,
                zalsa,
                active_query,
                last_provisional_memo_opt
                    .or(opt_old_memo)
                    .map(|memo| &memo.header),
            );

            let (mut active_query, cycle_heads, outer_cycle, cycle_iteration) =
                match try_complete_query(zalsa, active_query, claim_guard, iteration) {
                    QueryExecutionOutcome::Completed(completed_query) => {
                        break (new_value, completed_query);
                    }
                    QueryExecutionOutcome::Participant {
                        active_query,
                        cycle_heads,
                        outer_cycle,
                    } => {
                        // For FallbackImmediate, use the fallback value instead of the computed value
                        // for all cycle participants. This ensures that the results don't depend on the query call order, see
                        // https://github.com/salsa-rs/salsa/pull/798#issuecomment-2812855285.
                        if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                            new_value = C::cycle_initial(db, id, C::id_to_input(zalsa, id));
                        }

                        let completed_query = complete_cycle_participant(
                            active_query,
                            claim_guard,
                            cycle_heads,
                            outer_cycle,
                            iteration,
                        );

                        break (new_value, completed_query);
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
                let memo = self
                    .get_memo_from_table_for(zalsa, id, memo_ingredient_index)
                    .unwrap_or_else(|| {
                        unreachable!(
                            "{database_key_index:#?} is a cycle head, \
                                        but no provisional memo found"
                        )
                    });

                debug_assert!(memo.header.may_be_provisional());
                memo
            });

            let last_provisional_value = last_provisional_memo.value();

            let last_provisional_value = last_provisional_value.expect(
                "`fetch_cold_cycle` should have inserted a provisional memo with Cycle::initial",
            );
            tracing::debug!(
                "{database_key_index:?}: execute: \
                I am a cycle head, comparing last provisional value with new value"
            );

            // For FallbackImmediate, the value always converges immediately (we use the
            // fallback directly). We still iterate if metadata hasn't converged.
            // For Fixpoint, ask the recovery function what value to use and check convergence.
            let value_converged = if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                // Use the fallback value instead of the computed value.
                new_value = C::cycle_initial(db, id, C::id_to_input(zalsa, id));
                true
            } else {
                let cycle = Cycle {
                    head_ids: cycle_heads.ids(),
                    id,
                    iteration: cycle_iteration.iteration_as_u32(),
                };
                // We are in a cycle that hasn't converged; ask the user's
                // cycle-recovery function what to do (it may return the same value or a different one):
                new_value = C::recover_from_cycle(
                    db,
                    &cycle,
                    last_provisional_value,
                    new_value,
                    C::id_to_input(zalsa, id),
                );

                C::values_equal(&new_value, last_provisional_value)
            };

            let new_cycle_heads = active_query.take_cycle_heads();
            assert_no_new_cycle_heads(&cycle_heads, new_cycle_heads, database_key_index);

            let completed_query = match try_complete_cycle_head(
                active_query,
                claim_guard,
                cycle_heads,
                &last_provisional_memo.header.revisions,
                outer_cycle,
                iteration,
                cycle_iteration,
                value_converged,
            ) {
                Ok(completed_query) => {
                    break (new_value, completed_query);
                }
                Err((completed_query, new_iteration)) => {
                    iteration = new_iteration;
                    completed_query
                }
            };

            let new_memo = self.insert_memo(
                zalsa,
                id,
                Memo::new(Some(new_value), current_revision, completed_query.revisions),
                memo_ingredient_index,
            );

            last_provisional_memo_opt = Some(new_memo);

            last_stale_tracked_ids = completed_query.stale_tracked_structs;

            continue;
        };

        tracing::debug!(
            "{database_key_index:?}: execute_maybe_iterate: result.revisions = {revisions:#?}",
            revisions = &completed_query.revisions
        );

        (new_value, completed_query)
    }

    #[inline]
    fn execute_query<'db>(
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        active_query: ActiveQueryGuard<'db>,
        opt_old_header: Option<&MemoHeader>,
    ) -> (C::Output<'db>, ActiveQueryGuard<'db>) {
        if let Some(old_header) = opt_old_header {
            old_header.seed_active_query(zalsa, &active_query);
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let new_value = C::execute(
            db,
            C::id_to_input(zalsa, active_query.database_key_index.key_index()),
        );

        (new_value, active_query)
    }
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
        database_key_index: DatabaseKeyIndex,
        cancellation_count: u8,
        has_value: bool,
    ) -> Option<PreviousIteration> {
        if self.revisions.iteration().cancellation_count() != cancellation_count {
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

impl<'a, C: Configuration> PoisonProvisionalIfPanicking<'a, C> {
    fn new(
        ingredient: &'a IngredientImpl<C>,
        zalsa: &'a Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Self {
        Self {
            ingredient,
            zalsa,
            id,
            memo_ingredient_index,
        }
    }
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
            let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

            matches!(
                ingredient.wait_for(zalsa, head.database_key_index.key_index()),
                WaitForResult::Cycle { inner: false }
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

        let ingredient = zalsa.lookup_ingredient(current_head.ingredient_index());

        let provisional_status = ingredient
            .provisional_status(zalsa, current_head.key_index())
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
            let ingredient = zalsa.lookup_ingredient(database_key_index.ingredient_index());

            let converged = ingredient.cycle_converged(zalsa, database_key_index.key_index());

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
            let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());
            ingredient.finalize_cycle_head(zalsa, head.database_key_index.key_index());
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
        let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

        ingredient.set_cycle_iteration_count(zalsa, head.database_key_index.key_index(), iteration);
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

    if should_discard_retained_capacity(seen.len(), seen.capacity()) {
        seen = Default::default();
    } else {
        seen.clear();
    }
    let completion = detached_query.pop_completion(iteration, true);
    let completed_query = completion.finish(flattened.iter().copied());
    if should_discard_retained_capacity(flattened.len(), flattened.capacity()) {
        flattened = Default::default();
    } else {
        flattened.clear();
    }
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
