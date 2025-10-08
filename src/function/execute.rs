use smallvec::SmallVec;

use crate::active_query::CompletedQuery;
use crate::cycle::{CycleHeads, CycleRecoveryStrategy, IterationCount};
use crate::function::memo::Memo;
use crate::function::sync::ReleaseMode;
use crate::function::{ClaimGuard, Configuration, IngredientImpl};
use crate::ingredient::WaitForResult;
use crate::plumbing::ZalsaLocal;
use crate::sync::atomic::{AtomicBool, Ordering};
use crate::tracked_struct::Identity;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{ActiveQueryGuard, QueryRevisions};
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
    #[inline(never)]
    pub(super) fn execute<'db>(
        &'db self,
        db: &'db C::DbView,
        mut claim_guard: ClaimGuard<'db>,
        zalsa_local: &'db ZalsaLocal,
        opt_old_memo: Option<&Memo<'db, C>>,
    ) -> &'db Memo<'db, C> {
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
            CycleRecoveryStrategy::Panic => Self::execute_query(
                db,
                zalsa,
                zalsa_local.push_query(database_key_index, IterationCount::initial()),
                opt_old_memo,
            ),
            CycleRecoveryStrategy::FallbackImmediate => {
                let (mut new_value, mut completed_query) = Self::execute_query(
                    db,
                    zalsa,
                    zalsa_local.push_query(database_key_index, IterationCount::initial()),
                    opt_old_memo,
                );

                if let Some(cycle_heads) = completed_query.revisions.cycle_heads_mut() {
                    // Did the new result we got depend on our own provisional value, in a cycle?
                    if cycle_heads.contains(&database_key_index) {
                        let id = database_key_index.key_index();

                        // Ignore the computed value, leave the fallback value there.
                        let memo = self
                            .get_memo_from_table_for(zalsa, id, memo_ingredient_index)
                            .unwrap_or_else(|| {
                                unreachable!(
                                    "{database_key_index:#?} is a `FallbackImmediate` cycle head, \
                                        but no memo found"
                                )
                            });
                        // We need to mark the memo as finalized so other cycle participants that have fallbacks
                        // will be verified (participants that don't have fallbacks will not be verified).
                        memo.revisions.verified_final.store(true, Ordering::Release);
                        return memo;
                    }

                    // If we're in the middle of a cycle and we have a fallback, use it instead.
                    // Cycle participants that don't have a fallback will be discarded in
                    // `validate_provisional()`.
                    let cycle_heads = std::mem::take(cycle_heads);
                    let active_query =
                        zalsa_local.push_query(database_key_index, IterationCount::initial());
                    new_value = C::cycle_initial(db, C::id_to_input(zalsa, id));
                    completed_query = active_query.pop();
                    // We need to set `cycle_heads` and `verified_final` because it needs to propagate to the callers.
                    // When verifying this, we will see we have fallback and mark ourselves verified.
                    completed_query.revisions.set_cycle_heads(cycle_heads);
                    completed_query.revisions.verified_final = AtomicBool::new(false);
                }

                (new_value, completed_query)
            }
            CycleRecoveryStrategy::Fixpoint => self.execute_maybe_iterate(
                db,
                opt_old_memo,
                &mut claim_guard,
                zalsa_local,
                memo_ingredient_index,
            ),
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
            self.diff_outputs(zalsa, database_key_index, old_memo, &completed_query);
        }

        let new_memo = self.insert_memo(
            zalsa,
            database_key_index.key_index(),
            Memo::new(
                Some(new_value),
                zalsa.current_revision(),
                completed_query.revisions,
            ),
            memo_ingredient_index,
        );

        new_memo
    }

    fn execute_maybe_iterate<'db>(
        &'db self,
        db: &'db C::DbView,
        opt_old_memo: Option<&Memo<'db, C>>,
        claim_guard: &mut ClaimGuard<'db>,
        zalsa_local: &'db ZalsaLocal,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> (C::Output<'db>, CompletedQuery) {
        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();

        let id = database_key_index.key_index();

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // Initially it's set to None, because the initial provisional value is created lazily,
        // only when a cycle is actually encountered.
        let mut previous_memo: Option<&Memo<'db, C>> = None;
        // TODO: Can we seed those somehow?
        let mut last_stale_tracked_ids: Vec<(Identity, Id)> = Vec::new();

        let _guard = ClearCycleHeadIfPanicking::new(self, zalsa, id, memo_ingredient_index);
        let mut iteration_count = IterationCount::initial();

        if let Some(old_memo) = opt_old_memo {
            if old_memo.verified_at.load() == zalsa.current_revision()
                && old_memo.cycle_heads().contains(&database_key_index)
            {
                previous_memo = Some(old_memo);
                iteration_count = old_memo.revisions.iteration();
            }
        }

        let mut active_query = zalsa_local.push_query(database_key_index, iteration_count);
        claim_guard.set_release_mode(ReleaseMode::Default);

        let (new_value, completed_query) = loop {
            // Tracked struct ids that existed in the previous revision
            // but weren't recreated in the last iteration. It's important that we seed the next
            // query with these ids because the query might re-create them as part of the next iteration.
            // This is not only important to ensure that the re-created tracked structs have the same ids,
            // it's also important to ensure that these tracked structs get removed
            // if they aren't recreated when reaching the final iteration.
            active_query.seed_tracked_struct_ids(&last_stale_tracked_ids);

            let (mut new_value, mut completed_query) =
                Self::execute_query(db, zalsa, active_query, previous_memo);

            // If there are no cycle heads, break out of the loop (`cycle_heads_mut` returns `None` if the cycle head list is empty)
            let Some(cycle_heads) = completed_query.revisions.cycle_heads_mut() else {
                break (new_value, completed_query);
            };

            // TODO: Remove "removed" cycle heads"
            let mut cycle_heads = std::mem::take(cycle_heads);

            // Recursively resolve all cycle heads that this head depends on.
            // This isn't required in a single-threaded execution but it's not guaranteed that all nested cycles are listed
            // in cycle heads in a multi-threaded execution:
            //
            // t1: a -> b
            // t2: c -> b (blocks on t1)
            // t1: a -> b -> c (cycle, returns fixpoint initial with c(0) in heads)
            // t1: a -> b (completes b, b has c(0) in its cycle heads, releases `b`, which resumes `t2`, and `retry_provisional` blocks on `c` (t2))
            // t2: c -> a (cycle, returns fixpoint initial for a with a(0) in heads)
            // t2: completes c, `provisional_retry` blocks on `a` (t2)
            // t1: a (complets `b` with `c` in heads)
            //
            // Note how `a` only depends on `c` but not `a`. This is because `a` only saw the initial value of `c` and wasn't updated when `c` completed.
            // That's why we need to resolve the cycle heads recursively to `cycle_heads` contains all cycle heads at the moment this query completed.
            let mut queue: SmallVec<[DatabaseKeyIndex; 4]> = cycle_heads
                .iter()
                .map(|head| head.database_key_index)
                .filter(|head| *head != database_key_index)
                .collect();

            // TODO: Can we also resolve whether the cycles have converged here?
            while let Some(head) = queue.pop() {
                let ingredient = zalsa.lookup_ingredient(head.ingredient_index());
                let nested_heads = ingredient.cycle_heads(zalsa, head.key_index());

                for head in nested_heads {
                    if cycle_heads.insert(head) && !queue.contains(&head.database_key_index) {
                        queue.push(head.database_key_index);
                    }
                }
            }

            let outer_cycle = outer_cycle(zalsa, zalsa_local, &cycle_heads, database_key_index);

            // Did the new result we got depend on our own provisional value, in a cycle?
            if !cycle_heads.contains(&database_key_index) {
                if let Some(outer) = outer_cycle {
                    claim_guard.set_release_mode(ReleaseMode::TransferTo(outer));
                } else {
                    claim_guard.set_release_mode(ReleaseMode::SelfOnly);
                }

                completed_query.revisions.set_cycle_heads(cycle_heads);
                break (new_value, completed_query);
            }

            let last_provisional_value = if let Some(last_provisional) = previous_memo {
                // We have a last provisional value from our previous time around the loop.
                last_provisional.value.as_ref()
            } else {
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

                debug_assert!(memo.may_be_provisional());
                memo.value.as_ref()
            };

            let last_provisional_value = last_provisional_value.expect(
                "`fetch_cold_cycle` should have inserted a provisional memo with Cycle::initial",
            );
            crate::tracing::debug!(
                "{database_key_index:?}: execute: \
                        I am a cycle head, comparing last provisional value with new value"
            );

            // determine if it is a nested query.
            // This is a nested query if it depends on any other cycle head than itself
            // where claiming it results in a cycle. In that case, both queries form a single connected component
            // that we can iterate together rather than having separate nested fixpoint iterations.

            let this_converged = C::values_equal(&new_value, last_provisional_value);

            iteration_count = if outer_cycle.is_some() {
                iteration_count
            } else {
                cycle_heads
                    .iter()
                    .map(|head| head.iteration_count.load())
                    .max()
                    .unwrap_or(iteration_count)
            };

            // If the new result is equal to the last provisional result, the cycle has
            // converged and we are done.
            if !this_converged {
                // We are in a cycle that hasn't converged; ask the user's
                // cycle-recovery function what to do:
                match C::recover_from_cycle(
                    db,
                    &new_value,
                    iteration_count.as_u32(),
                    C::id_to_input(zalsa, id),
                ) {
                    crate::CycleRecoveryAction::Iterate => {}
                    crate::CycleRecoveryAction::Fallback(fallback_value) => {
                        crate::tracing::debug!(
                            "{database_key_index:?}: execute: user cycle_fn says to fall back"
                        );
                        new_value = fallback_value;
                    }
                }
            }

            completed_query
                .revisions
                .set_cycle_converged(this_converged);

            if let Some(outer_cycle) = outer_cycle {
                tracing::debug!(
                    "Detected nested cycle {database_key_index:?}, iterate it as part of the outer cycle {outer_cycle:?}"
                );

                completed_query.revisions.set_cycle_heads(cycle_heads);
                claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));

                break (new_value, completed_query);
            }

            // Verify that all cycles have converged, including all inner cycles.
            let converged = this_converged
                && cycle_heads
                    .iter()
                    .filter(|head| head.database_key_index != database_key_index)
                    .all(|head| {
                        let ingredient =
                            zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

                        let converged =
                            ingredient.cycle_converged(zalsa, head.database_key_index.key_index());

                        if !converged {
                            tracing::debug!("inner cycle {database_key_index:?} has not converged");
                        }

                        converged
                    });

            if converged {
                crate::tracing::debug!(
                        "{database_key_index:?}: execute: fixpoint iteration has a final value after {iteration_count:?} iterations"
                    );

                // Set the nested cycles as verified. This is necessary because
                // `validate_provisional` doesn't follow cycle heads recursively (and the inner memos now depend on all cycle heads).
                for head in cycle_heads {
                    if head.database_key_index == database_key_index {
                        continue;
                    }

                    let ingredient =
                        zalsa.lookup_ingredient(head.database_key_index.ingredient_index());
                    ingredient.finalize_cycle_head(zalsa, head.database_key_index.key_index());
                }

                *completed_query.revisions.verified_final.get_mut() = true;

                break (new_value, completed_query);
            }

            completed_query.revisions.set_cycle_heads(cycle_heads);

            // `iteration_count` can't overflow as we check it against `MAX_ITERATIONS`
            // which is less than `u32::MAX`.
            iteration_count = iteration_count.increment().unwrap_or_else(|| {
                tracing::warn!("{database_key_index:?}: execute: too many cycle iterations");
                panic!("{database_key_index:?}: execute: too many cycle iterations")
            });

            zalsa.event(&|| {
                Event::new(EventKind::WillIterateCycle {
                    database_key: database_key_index,
                    iteration_count,
                })
            });

            crate::tracing::info!(
                "{database_key_index:?}: execute: iterate again ({iteration_count:?})...",
            );

            completed_query
                .revisions
                .update_iteration_count_mut(database_key_index, iteration_count);

            for head in completed_query.revisions.cycle_heads() {
                if head.database_key_index == database_key_index {
                    continue;
                }

                let ingredient =
                    zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

                // let iteration_count = if was_initial && !head.iteration_count.load().is_initial() {
                //     IterationCount::first_after_restart()
                // } else {
                //     iteration_count
                // };

                ingredient.set_cycle_iteration_count(
                    zalsa,
                    head.database_key_index.key_index(),
                    iteration_count,
                );
            }

            let new_memo = self.insert_memo(
                zalsa,
                id,
                Memo::new(
                    Some(new_value),
                    zalsa.current_revision(),
                    completed_query.revisions,
                ),
                memo_ingredient_index,
            );

            previous_memo = Some(new_memo);

            last_stale_tracked_ids = completed_query.stale_tracked_structs;
            active_query = zalsa_local.push_query(database_key_index, iteration_count);

            continue;
        };

        crate::tracing::debug!(
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
        opt_old_memo: Option<&Memo<'db, C>>,
    ) -> (C::Output<'db>, CompletedQuery) {
        if let Some(old_memo) = opt_old_memo {
            // If we already executed this query once, then use the tracked-struct ids from the
            // previous execution as the starting point for the new one.
            active_query.seed_tracked_struct_ids(old_memo.revisions.tracked_struct_ids());

            // Copy over all inputs and outputs from a previous iteration.
            // This is necessary to:
            // * ensure that tracked struct created during the previous iteration
            //   (and are owned by the query) are alive even if the query in this iteration no longer creates them.
            // * ensure the final returned memo depends on all inputs from all iterations.
            if old_memo.may_be_provisional()
                && old_memo.verified_at.load() == zalsa.current_revision()
            {
                active_query.seed_iteration(&old_memo.revisions);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let new_value = C::execute(
            db,
            C::id_to_input(zalsa, active_query.database_key_index.key_index()),
        );

        (new_value, active_query.pop())
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
struct ClearCycleHeadIfPanicking<'a, C: Configuration> {
    ingredient: &'a IngredientImpl<C>,
    zalsa: &'a Zalsa,
    id: Id,
    memo_ingredient_index: MemoIngredientIndex,
}

impl<'a, C: Configuration> ClearCycleHeadIfPanicking<'a, C> {
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

impl<C: Configuration> Drop for ClearCycleHeadIfPanicking<'_, C> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut revisions =
                QueryRevisions::fixpoint_initial(self.ingredient.database_key_index(self.id));
            revisions.update_iteration_count_mut(
                self.ingredient.database_key_index(self.id),
                IterationCount::panicked(),
            );

            let memo = Memo::new(None, self.zalsa.current_revision(), revisions);
            self.ingredient
                .insert_memo(self.zalsa, self.id, memo, self.memo_ingredient_index);
        }
    }
}

fn outer_cycle(
    zalsa: &Zalsa,
    zalsa_local: &ZalsaLocal,
    cycle_heads: &CycleHeads,
    current_key: DatabaseKeyIndex,
) -> Option<DatabaseKeyIndex> {
    cycle_heads
        .iter()
        .filter(|head| head.database_key_index != current_key)
        .find(|head| {
            // SAFETY: We don't call into with_query_stack recursively
            let is_on_stack = unsafe {
                zalsa_local.with_query_stack_unchecked(|stack| {
                    stack
                        .iter()
                        .rev()
                        .any(|query| query.database_key_index == head.database_key_index)
                })
            };

            if is_on_stack {
                return true;
            }

            let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

            matches!(
                ingredient.wait_for(zalsa, head.database_key_index.key_index()),
                WaitForResult::Cycle { inner: false }
            )
        })
        .map(|head| head.database_key_index)
}
