use crate::cycle::{CycleRecoveryStrategy, IterationCount};
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::sync::atomic::{AtomicBool, Ordering};
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::{ActiveQueryGuard, QueryRevisions};
use crate::{Event, EventKind, Id, Revision};

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
        active_query: ActiveQueryGuard<'db>,
        opt_old_memo: Option<&Memo<'db, C>>,
    ) -> &'db Memo<'db, C> {
        let database_key_index = active_query.database_key_index;
        let id = database_key_index.key_index();

        crate::tracing::info!("{:?}: executing query", database_key_index);
        let zalsa = db.zalsa();

        zalsa.event(&|| {
            Event::new(EventKind::WillExecute {
                database_key: database_key_index,
            })
        });
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

        let (new_value, mut revisions) = match C::CYCLE_STRATEGY {
            CycleRecoveryStrategy::Panic => {
                Self::execute_query(db, active_query, opt_old_memo, zalsa.current_revision(), id)
            }
            CycleRecoveryStrategy::FallbackImmediate => {
                let (mut new_value, mut revisions) = Self::execute_query(
                    db,
                    active_query,
                    opt_old_memo,
                    zalsa.current_revision(),
                    id,
                );

                if let Some(cycle_heads) = revisions.cycle_heads_mut() {
                    // Did the new result we got depend on our own provisional value, in a cycle?
                    if cycle_heads.contains(&database_key_index) {
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
                    let active_query = db
                        .zalsa_local()
                        .push_query(database_key_index, IterationCount::initial());
                    new_value = C::cycle_initial(db, C::id_to_input(db, id));
                    revisions = active_query.pop();
                    // We need to set `cycle_heads` and `verified_final` because it needs to propagate to the callers.
                    // When verifying this, we will see we have fallback and mark ourselves verified.
                    revisions.set_cycle_heads(cycle_heads);
                    revisions.verified_final = AtomicBool::new(false);
                }

                (new_value, revisions)
            }
            CycleRecoveryStrategy::Fixpoint => self.execute_maybe_iterate(
                db,
                active_query,
                opt_old_memo,
                zalsa,
                id,
                memo_ingredient_index,
            ),
        };

        if let Some(old_memo) = opt_old_memo {
            // If the new value is equal to the old one, then it didn't
            // really change, even if some of its inputs have. So we can
            // "backdate" its `changed_at` revision to be the same as the
            // old value.
            self.backdate_if_appropriate(old_memo, database_key_index, &mut revisions, &new_value);

            // Diff the new outputs with the old, to discard any no-longer-emitted
            // outputs and update the tracked struct IDs for seeding the next revision.
            self.diff_outputs(zalsa, database_key_index, old_memo, &mut revisions);
        }
        self.insert_memo(
            zalsa,
            id,
            Memo::new(Some(new_value), zalsa.current_revision(), revisions),
            memo_ingredient_index,
        )
    }

    #[inline]
    fn execute_maybe_iterate<'db>(
        &'db self,
        db: &'db C::DbView,
        mut active_query: ActiveQueryGuard<'db>,
        opt_old_memo: Option<&Memo<'db, C>>,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> (C::Output<'db>, QueryRevisions) {
        let database_key_index = active_query.database_key_index;
        let mut iteration_count = IterationCount::initial();
        let mut fell_back = false;

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // Initially it's set to None, because the initial provisional value is created lazily,
        // only when a cycle is actually encountered.
        let mut opt_last_provisional: Option<&Memo<'db, C>> = None;
        loop {
            let previous_memo = opt_last_provisional.or(opt_old_memo);
            let (mut new_value, mut revisions) = Self::execute_query(
                db,
                active_query,
                previous_memo,
                zalsa.current_revision(),
                id,
            );

            // Did the new result we got depend on our own provisional value, in a cycle?
            if let Some(cycle_heads) = revisions
                .cycle_heads_mut()
                .filter(|cycle_heads| cycle_heads.contains(&database_key_index))
            {
                let last_provisional_value = if let Some(last_provisional) = opt_last_provisional {
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
                // SAFETY: The `LRU` does not run mid-execution, so the value remains filled
                let last_provisional_value = unsafe { last_provisional_value.unwrap_unchecked() };
                crate::tracing::debug!(
                    "{database_key_index:?}: execute: \
                        I am a cycle head, comparing last provisional value with new value"
                );
                // If the new result is equal to the last provisional result, the cycle has
                // converged and we are done.
                if !C::values_equal(&new_value, last_provisional_value) {
                    if fell_back {
                        // We fell back to a value last iteration, but the fallback didn't result
                        // in convergence. We only have bad options here: continue iterating
                        // (ignoring the request to fall back), or forcibly use the fallback and
                        // leave the cycle in an inconsistent state (we'll be using a value for
                        // this query that it doesn't evaluate to, given its inputs). Maybe we'll
                        // have to go with the latter, but for now let's panic and see if real use
                        // cases need non-converging fallbacks.
                        panic!("{database_key_index:?}: execute: fallback did not converge");
                    }
                    // We are in a cycle that hasn't converged; ask the user's
                    // cycle-recovery function what to do:
                    match C::recover_from_cycle(
                        db,
                        &new_value,
                        iteration_count.as_u32(),
                        C::id_to_input(db, id),
                    ) {
                        crate::CycleRecoveryAction::Iterate => {}
                        crate::CycleRecoveryAction::Fallback(fallback_value) => {
                            crate::tracing::debug!(
                                "{database_key_index:?}: execute: user cycle_fn says to fall back"
                            );
                            new_value = fallback_value;
                            // We have to insert the fallback value for this query and then iterate
                            // one more time to fill in correct values for everything else in the
                            // cycle based on it; then we'll re-insert it as final value.
                            fell_back = true;
                        }
                    }
                    // `iteration_count` can't overflow as we check it against `MAX_ITERATIONS`
                    // which is less than `u32::MAX`.
                    iteration_count = iteration_count.increment().unwrap_or_else(|| {
                        panic!("{database_key_index:?}: execute: too many cycle iterations")
                    });
                    zalsa.event(&|| {
                        Event::new(EventKind::WillIterateCycle {
                            database_key: database_key_index,
                            iteration_count,
                            fell_back,
                        })
                    });
                    cycle_heads.update_iteration_count(database_key_index, iteration_count);
                    revisions.update_iteration_count(iteration_count);
                    crate::tracing::debug!(
                        "{database_key_index:?}: execute: iterate again, revisions: {revisions:#?}"
                    );
                    opt_last_provisional = Some(self.insert_memo(
                        zalsa,
                        id,
                        Memo::new(Some(new_value), zalsa.current_revision(), revisions),
                        memo_ingredient_index,
                    ));

                    active_query = db
                        .zalsa_local()
                        .push_query(database_key_index, iteration_count);

                    continue;
                }
                crate::tracing::debug!(
                    "{database_key_index:?}: execute: fixpoint iteration has a final value"
                );
                cycle_heads.remove(&database_key_index);

                if cycle_heads.is_empty() {
                    // If there are no more cycle heads, we can mark this as verified.
                    revisions.verified_final.store(true, Ordering::Relaxed);
                }
            }

            crate::tracing::debug!(
                "{database_key_index:?}: execute: result.revisions = {revisions:#?}"
            );

            break (new_value, revisions);
        }
    }

    #[inline]
    fn execute_query<'db>(
        db: &'db C::DbView,
        active_query: ActiveQueryGuard<'db>,
        opt_old_memo: Option<&Memo<'db, C>>,
        current_revision: Revision,
        id: Id,
    ) -> (C::Output<'db>, QueryRevisions) {
        if let Some(old_memo) = opt_old_memo {
            // If we already executed this query once, then use the tracked-struct ids from the
            // previous execution as the starting point for the new one.
            if let Some(tracked_struct_ids) = old_memo.revisions.tracked_struct_ids() {
                active_query.seed_tracked_struct_ids(tracked_struct_ids);
            }

            // Copy over all inputs and outputs from a previous iteration.
            // This is necessary to:
            // * ensure that tracked struct created during the previous iteration
            //   (and are owned by the query) are alive even if the query in this iteration no longer creates them.
            // * ensure the final returned memo depends on all inputs from all iterations.
            if old_memo.may_be_provisional() && old_memo.verified_at.load() == current_revision {
                active_query.seed_iteration(&old_memo.revisions);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let new_value = C::execute(db, C::id_to_input(db, id));

        (new_value, active_query.pop())
    }
}
