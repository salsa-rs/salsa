use std::sync::atomic::Ordering;

use crate::cycle::{CycleRecoveryStrategy, MAX_ITERATIONS};
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::zalsa::ZalsaDatabase;
use crate::zalsa_local::ActiveQueryGuard;
use crate::{Database, Event, EventKind};

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
    pub(super) fn execute<'db>(
        &'db self,
        db: &'db C::DbView,
        mut active_query: ActiveQueryGuard<'db>,
        opt_old_memo: Option<&Memo<C::Output<'db>>>,
    ) -> &'db Memo<C::Output<'db>> {
        let zalsa = db.zalsa();
        let revision_now = zalsa.current_revision();
        let database_key_index = active_query.database_key_index;
        let id = database_key_index.key_index();

        tracing::info!("{:?}: executing query", database_key_index);

        db.salsa_event(&|| {
            Event::new(EventKind::WillExecute {
                database_key: database_key_index,
            })
        });

        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

        let mut iteration_count: u32 = 0;
        let mut fell_back = false;

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // Initially it's set to None, because the initial provisional value is created lazily,
        // only when a cycle is actually encountered.
        let mut opt_last_provisional: Option<&Memo<<C as Configuration>::Output<'db>>> = None;

        loop {
            // If we already executed this query once, then use the tracked-struct ids from the
            // previous execution as the starting point for the new one.
            if let Some(old_memo) = opt_old_memo {
                active_query.seed_tracked_struct_ids(&old_memo.revisions.tracked_struct_ids);

                // Copy over all outputs from a previous iteration.
                // This is necessary to ensure that tracked struct created during the previous iteration
                // (and are owned by the query) alive even if the query in this iteratoin no longer creates them.
                // The query not re-creating the tracked struct doesn't guarantee that there
                // aren't any other queries depending on it.
                if old_memo.verified_at.load() == revision_now && old_memo.may_be_provisional() {
                    for output in old_memo.revisions.origin.outputs() {
                        active_query.add_output(output);
                    }
                }
            }

            // Query was not previously executed, or value is potentially
            // stale, or value is absent. Let's execute!
            let mut new_value = C::execute(db, C::id_to_input(db, id));
            let mut revisions = active_query.pop();

            // Did the new result we got depend on our own provisional value, in a cycle?
            if revisions.cycle_heads.contains(&database_key_index) {
                if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
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
                    // SAFETY: This is ours memo.
                    return unsafe { self.extend_memo_lifetime(memo) };
                } else if C::CYCLE_STRATEGY == CycleRecoveryStrategy::Fixpoint {
                    let last_provisional_value =
                        if let Some(last_provisional) = opt_last_provisional {
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
                    let last_provisional_value =
                        unsafe { last_provisional_value.unwrap_unchecked() };
                    tracing::debug!(
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
                            iteration_count,
                            C::id_to_input(db, id),
                        ) {
                            crate::CycleRecoveryAction::Iterate => {
                                tracing::debug!("{database_key_index:?}: execute: iterate again");
                            }
                            crate::CycleRecoveryAction::Fallback(fallback_value) => {
                                tracing::debug!(
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
                        iteration_count += 1;
                        if iteration_count > MAX_ITERATIONS {
                            panic!("{database_key_index:?}: execute: too many cycle iterations");
                        }
                        db.salsa_event(&|| {
                            Event::new(EventKind::WillIterateCycle {
                                database_key: database_key_index,
                                iteration_count,
                                fell_back,
                            })
                        });
                        revisions
                            .cycle_heads
                            .update_iteration_count(database_key_index, iteration_count);
                        opt_last_provisional = Some(self.insert_memo(
                            zalsa,
                            id,
                            Memo::new(Some(new_value), revision_now, revisions),
                            memo_ingredient_index,
                        ));

                        active_query = db
                            .zalsa_local()
                            .push_query(database_key_index, iteration_count);

                        continue;
                    }
                    tracing::debug!(
                        "{database_key_index:?}: execute: fixpoint iteration has a final value"
                    );
                    revisions.cycle_heads.remove(&database_key_index);
                }
            }

            tracing::debug!("{database_key_index:?}: execute: result.revisions = {revisions:#?}");

            if !revisions.cycle_heads.is_empty()
                && C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate
            {
                // If we're in the middle of a cycle and we have a fallback, use it instead.
                // Cycle participants that don't have a fallback will be discarded in
                // `validate_provisional()`.
                let cycle_heads = revisions.cycle_heads;
                let active_query = db.zalsa_local().push_query(database_key_index, 0);
                new_value = C::cycle_initial(db, C::id_to_input(db, id));
                revisions = active_query.pop();
                // We need to set `cycle_heads` and `verified_final` because it needs to propagate to the callers.
                // When verifying this, we will see we have fallback and mark ourselves verified.
                revisions.cycle_heads = cycle_heads;
                *revisions.verified_final.get_mut() = false;
            }

            if let Some(old_memo) = opt_old_memo {
                // If the new value is equal to the old one, then it didn't
                // really change, even if some of its inputs have. So we can
                // "backdate" its `changed_at` revision to be the same as the
                // old value.
                self.backdate_if_appropriate(old_memo, &mut revisions, &new_value);

                // Diff the new outputs with the old, to discard any no-longer-emitted
                // outputs and update the tracked struct IDs for seeding the next revision.
                let provisional = !revisions.cycle_heads.is_empty();
                self.diff_outputs(
                    zalsa,
                    db,
                    database_key_index,
                    old_memo,
                    &mut revisions,
                    provisional,
                );
            }

            return self.insert_memo(
                zalsa,
                id,
                Memo::new(Some(new_value), revision_now, revisions),
                memo_ingredient_index,
            );
        }
    }
}
