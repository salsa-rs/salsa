use std::sync::Arc;

use crate::{
    zalsa::ZalsaDatabase, zalsa_local::QueryRevisions, Database, DatabaseKeyIndex, Event, EventKind,
};

use super::{memo::Memo, Configuration, IngredientImpl};

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
    /// * `opt_old_memo`, the older memo, if any existed. Used for backdated.
    pub(super) fn execute<'db>(
        &'db self,
        db: &'db C::DbView,
        database_key_index: DatabaseKeyIndex,
        opt_old_memo: Option<Arc<Memo<C::Output<'_>>>>,
    ) -> &'db Memo<C::Output<'db>> {
        let (zalsa, zalsa_local) = db.zalsas();
        let revision_now = zalsa.current_revision();
        let id = database_key_index.key_index;

        tracing::info!("{:?}: executing query", database_key_index);

        db.salsa_event(&|| Event {
            thread_id: std::thread::current().id(),
            kind: EventKind::WillExecute {
                database_key: database_key_index,
            },
        });

        let mut opt_last_provisional = self.initial_value(db).map(|initial_value| {
            self.insert_memo(
                zalsa,
                id,
                Memo::new(
                    Some(initial_value),
                    revision_now,
                    QueryRevisions::fixpoint_initial(database_key_index),
                ),
            )
        });

        let mut iteration_count = 0;

        loop {
            let active_query = zalsa_local.push_query(database_key_index);

            // If we already executed this query once, then use the tracked-struct ids from the
            // previous execution as the starting point for the new one.
            if let Some(old_memo) = &opt_old_memo {
                active_query.seed_tracked_struct_ids(&old_memo.revisions.tracked_struct_ids);
            }

            // Query was not previously executed, or value is potentially
            // stale, or value is absent. Let's execute!
            let mut new_value = C::execute(db, C::id_to_input(db, id));
            let mut revisions = active_query.pop();

            // If the new value is equal to the old one, then it didn't
            // really change, even if some of its inputs have. So we can
            // "backdate" its `changed_at` revision to be the same as the
            // old value.
            if let Some(old_memo) = &opt_old_memo {
                self.backdate_if_appropriate(old_memo, &mut revisions, &new_value);
                self.diff_outputs(db, database_key_index, old_memo, &mut revisions);
            }

            // Did the new result we got depend on our own provisional value, in a cycle?
            if revisions.cycle_heads.contains(&database_key_index) {
                if let Some(last_provisional) = opt_last_provisional {
                    if let Some(provisional_value) = &last_provisional.value {
                        // If the new result is equal to the last provisional result, the cycle has
                        // converged and we are done.
                        if !C::values_equal(&new_value, provisional_value) {
                            // We are in a cycle that hasn't converged; ask the user's
                            // cycle-recovery function what to do:
                            match C::recover_from_cycle(db, &new_value, iteration_count) {
                                crate::CycleRecoveryAction::Iterate => {
                                    iteration_count += 1;
                                    revisions.cycle_ignore = false;
                                    opt_last_provisional = Some(self.insert_memo(
                                        zalsa,
                                        id,
                                        Memo::new(Some(new_value), revision_now, revisions),
                                    ));
                                    continue;
                                }
                                crate::CycleRecoveryAction::Fallback(fallback_value) => {
                                    new_value = fallback_value;
                                }
                            }
                        }
                    }
                }
                // This is no longer a provisional result, it's our final result, so remove ourself
                // from the cycle heads, and iterate one last time to remove ourself from all other
                // results in the cycle as well.
                revisions.cycle_heads.remove(&database_key_index);
                revisions.cycle_ignore = false;
                self.insert_memo(
                    zalsa,
                    id,
                    Memo::new(Some(new_value), revision_now, revisions),
                );
                continue;
            }
            return self.insert_memo(
                zalsa,
                id,
                Memo::new(Some(new_value), revision_now, revisions),
            );
        }
    }
}
