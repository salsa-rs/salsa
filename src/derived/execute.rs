use std::sync::Arc;

use crate::{
    plumbing::QueryFunction,
    runtime::{local_state::ActiveQueryGuard, StampedValue},
    Cycle, Database, Event, EventKind, QueryDb,
};

use super::{memo::Memo, DerivedStorage, MemoizationPolicy};

impl<Q, MP> DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
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
    pub(super) fn execute(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        active_query: ActiveQueryGuard<'_>,
        opt_old_memo: Option<Arc<Memo<Q::Value>>>,
    ) -> StampedValue<Q::Value> {
        let runtime = db.salsa_runtime();
        let revision_now = runtime.current_revision();
        let database_key_index = active_query.database_key_index;

        log::info!("{:?}: executing query", database_key_index.debug(db));

        db.salsa_event(Event {
            runtime_id: db.salsa_runtime().id(),
            kind: EventKind::WillExecute {
                database_key: database_key_index,
            },
        });

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let database_key_index = active_query.database_key_index;
        let key_index = database_key_index.key_index;
        let key = self.key_map.key_for_key_index(key_index);
        let value = match Cycle::catch(|| Q::execute(db, key.clone())) {
            Ok(v) => v,
            Err(cycle) => {
                log::debug!(
                    "{:?}: caught cycle {:?}, have strategy {:?}",
                    database_key_index.debug(db),
                    cycle,
                    Q::CYCLE_STRATEGY,
                );
                match Q::CYCLE_STRATEGY {
                    crate::plumbing::CycleRecoveryStrategy::Panic => cycle.throw(),
                    crate::plumbing::CycleRecoveryStrategy::Fallback => {
                        if let Some(c) = active_query.take_cycle() {
                            assert!(c.is(&cycle));
                            Q::cycle_fallback(db, &cycle, &key)
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
        if let Some(old_memo) = &opt_old_memo {
            if let Some(old_value) = &old_memo.value {
                // Careful: if the value became less durable than it
                // used to be, that is a "breaking change" that our
                // consumers must be aware of. Becoming *more* durable
                // is not. See the test `constant_to_non_constant`.
                if revisions.durability >= old_memo.revisions.durability
                    && MP::memoized_value_eq(old_value, &value)
                {
                    log::debug!(
                        "{:?}: read_upgrade: value is equal, back-dating to {:?}",
                        database_key_index.debug(db),
                        old_memo.revisions.changed_at,
                    );

                    assert!(old_memo.revisions.changed_at <= revisions.changed_at);
                    revisions.changed_at = old_memo.revisions.changed_at;
                }
            }
        }

        let stamped_value = revisions.stamped_value(value);

        log::debug!(
            "{:?}: read_upgrade: result.revisions = {:#?}",
            database_key_index.debug(db),
            revisions
        );

        self.memo_map.insert(
            key_index,
            Memo::new(
                if MP::should_memoize_value(&key) {
                    Some(stamped_value.value.clone())
                } else {
                    None
                },
                revision_now,
                revisions,
            ),
        );

        stamped_value
    }
}
