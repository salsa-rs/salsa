use crate::active_query::CompletedQuery;
use crate::cycle::{CycleRecoveryStrategy, IterationCount};
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
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
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        opt_old_memo: Option<&Memo<'db, C>>,
    ) -> &'db Memo<'db, C> {
        let id = database_key_index.key_index();

        crate::tracing::info!("{:?}: executing query", database_key_index);

        zalsa.event(&|| {
            Event::new(EventKind::WillExecute {
                database_key: database_key_index,
            })
        });
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

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
                zalsa,
                zalsa_local,
                database_key_index,
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
        self.insert_memo(
            zalsa,
            id,
            Memo::new(
                Some(new_value),
                zalsa.current_revision(),
                completed_query.revisions,
            ),
            memo_ingredient_index,
        )
    }

    fn execute_maybe_iterate<'db>(
        &'db self,
        db: &'db C::DbView,
        opt_old_memo: Option<&Memo<'db, C>>,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> (C::Output<'db>, CompletedQuery) {
        let id = database_key_index.key_index();
        let mut iteration_count = IterationCount::initial();
        let mut active_query = zalsa_local.push_query(database_key_index, iteration_count);

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // Initially it's set to None, because the initial provisional value is created lazily,
        // only when a cycle is actually encountered.
        let mut opt_last_provisional: Option<&Memo<'db, C>> = None;
        let mut last_stale_tracked_ids: Vec<(Identity, Id)> = Vec::new();
        let _guard = ClearCycleHeadIfPanicking::new(self, zalsa, id, memo_ingredient_index);

        loop {
            let previous_memo = opt_last_provisional.or(opt_old_memo);

            // Tracked struct ids that existed in the previous revision
            // but weren't recreated in the last iteration. It's important that we seed the next
            // query with these ids because the query might re-create them as part of the next iteration.
            // This is not only important to ensure that the re-created tracked structs have the same ids,
            // it's also important to ensure that these tracked structs get removed
            // if they aren't recreated when reaching the final iteration.
            active_query.seed_tracked_struct_ids(&last_stale_tracked_ids);

            let (mut new_value, mut completed_query) =
                Self::execute_query(db, zalsa, active_query, previous_memo);

            // Did the new result we got depend on our own provisional value, in a cycle?
            if let Some(cycle_heads) = completed_query
                .revisions
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
                        .filter(|memo| memo.verified_at.load() == zalsa.current_revision())
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
                // If the new result is equal to the last provisional result, the cycle has
                // converged and we are done.
                if !C::values_equal(&new_value, last_provisional_value) {
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
                    // `iteration_count` can't overflow as we check it against `MAX_ITERATIONS`
                    // which is less than `u32::MAX`.
                    iteration_count = iteration_count.increment().unwrap_or_else(|| {
                        tracing::warn!(
                            "{database_key_index:?}: execute: too many cycle iterations"
                        );
                        panic!("{database_key_index:?}: execute: too many cycle iterations")
                    });
                    zalsa.event(&|| {
                        Event::new(EventKind::WillIterateCycle {
                            database_key: database_key_index,
                            iteration_count,
                        })
                    });
                    cycle_heads.update_iteration_count(database_key_index, iteration_count);
                    completed_query
                        .revisions
                        .update_iteration_count(iteration_count);
                    crate::tracing::info!("{database_key_index:?}: execute: iterate again...",);
                    opt_last_provisional = Some(self.insert_memo(
                        zalsa,
                        id,
                        Memo::new(
                            Some(new_value),
                            zalsa.current_revision(),
                            completed_query.revisions,
                        ),
                        memo_ingredient_index,
                    ));
                    last_stale_tracked_ids = completed_query.stale_tracked_structs;

                    active_query = zalsa_local.push_query(database_key_index, iteration_count);

                    continue;
                }
                crate::tracing::debug!(
                    "{database_key_index:?}: execute: fixpoint iteration has a final value"
                );
                cycle_heads.remove(&database_key_index);

                if cycle_heads.is_empty() {
                    // If there are no more cycle heads, we can mark this as verified.
                    completed_query
                        .revisions
                        .verified_final
                        .store(true, Ordering::Relaxed);
                }
            }

            crate::tracing::debug!(
                "{database_key_index:?}: execute: result.revisions = {revisions:#?}",
                revisions = &completed_query.revisions
            );

            break (new_value, completed_query);
        }
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
            let revisions =
                QueryRevisions::fixpoint_initial(self.ingredient.database_key_index(self.id));

            let memo = Memo::new(None, self.zalsa.current_revision(), revisions);
            self.ingredient
                .insert_memo(self.zalsa, self.id, memo, self.memo_ingredient_index);
        }
    }
}
