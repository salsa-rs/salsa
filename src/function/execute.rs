use std::sync::Arc;

use crate::{
    runtime::StampedValue, zalsa::ZalsaDatabase, zalsa_local::ActiveQueryGuard, Cycle, Database,
    Event, EventKind,
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
        active_query: ActiveQueryGuard<'_>,
        opt_old_memo: Option<Arc<Memo<C::Output<'_>>>>,
    ) -> StampedValue<&C::Output<'db>> {
        let zalsa = db.zalsa();
        let revision_now = zalsa.current_revision();
        let database_key_index = active_query.database_key_index;

        tracing::info!("{:?}: executing query", database_key_index);

        db.salsa_event(&|| Event {
            thread_id: std::thread::current().id(),
            kind: EventKind::WillExecute {
                database_key: database_key_index,
            },
        });

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let database_key_index = active_query.database_key_index;
        let key = database_key_index.key_index;
        let value = match Cycle::catch(|| C::execute(db, C::id_to_input(db, key))) {
            Ok(v) => v,
            Err(cycle) => {
                tracing::debug!(
                    "{database_key_index:?}: caught cycle {cycle:?}, have strategy {:?}",
                    C::CYCLE_STRATEGY
                );
                match C::CYCLE_STRATEGY {
                    crate::cycle::CycleRecoveryStrategy::Panic => cycle.throw(),
                    crate::cycle::CycleRecoveryStrategy::Fallback => {
                        if let Some(c) = active_query.take_cycle() {
                            assert!(c.is(&cycle));
                            C::recover_from_cycle(db, &cycle, C::id_to_input(db, key))
                        } else {
                            // we are not a participant in this cycle
                            debug_assert!(!cycle
                                .participant_keys()
                                .any(|k| k == database_key_index));
                            cycle.throw()
                        }
                    }
                }
            }
        };
        let mut revisions = active_query.pop();

        // If the new value is equal to the old one, then it didn't
        // really change, even if some of its inputs have. So we can
        // "backdate" its `changed_at` revision to be the same as the
        // old value.
        if let Some(old_memo) = &opt_old_memo {
            self.backdate_if_appropriate(old_memo, &mut revisions, &value);
            self.diff_outputs(db, database_key_index, old_memo, &revisions);
        }

        let value = self
            .insert_memo(
                db,
                key,
                Memo::new(Some(value), revision_now, revisions.clone()),
            )
            .unwrap();

        let stamped_value = revisions.stamped_value(value);

        tracing::debug!("{database_key_index:?}: read_upgrade: result.revisions = {revisions:#?}");

        stamped_value
    }
}
