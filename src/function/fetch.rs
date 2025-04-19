use crate::cycle::CycleRecoveryStrategy;
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl, VerifyResult};
use crate::table::sync::ClaimResult;
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::{ActiveQueryGuard, QueryRevisions, ZalsaLocal};
use crate::{DatabaseKeyIndex, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, id: Id) -> &'db C::Output<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        zalsa.unwind_if_revision_cancelled(db);

        let memo = self.refresh_memo(db, zalsa, id);
        // SAFETY: We just refreshed the memo so it is guaranteed to contain a value now.
        let memo_value = unsafe { memo.value.as_ref().unwrap_unchecked() };

        self.lru.record_use(id);

        zalsa_local.report_tracked_read(
            self.database_key_index(id),
            memo.revisions.durability,
            memo.revisions.changed_at,
            memo.revisions.accumulated.is_some(),
            &memo.revisions.accumulated_inputs,
            memo.cycle_heads(),
        );

        memo_value
    }

    #[inline(always)]
    pub(super) fn refresh_memo<'db>(
        &'db self,
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        id: Id,
    ) -> &'db Memo<C::Output<'db>> {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);
        loop {
            if let Some(memo) = self
                .fetch_hot(zalsa, db, id, memo_ingredient_index)
                .or_else(|| self.fetch_cold(zalsa, db, id, memo_ingredient_index))
            {
                // If we get back a provisional cycle memo, and it's provisional on any cycle heads
                // that are claimed by a different thread, we can't propagate the provisional memo
                // any further (it could escape outside the cycle); we need to block on the other
                // thread completing fixpoint iteration of the cycle, and then we can re-query for
                // our no-longer-provisional memo.
                if !memo.provisional_retry(db, zalsa, self.database_key_index(id)) {
                    return memo;
                }
            }
        }
    }

    #[inline]
    fn fetch_hot<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        db: &'db C::DbView,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C::Output<'db>>> {
        let memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index)?;

        memo.value.as_ref()?;

        let database_key_index = self.database_key_index(id);

        let shallow_update = self.shallow_verify_memo(zalsa, database_key_index, memo)?;

        if self.validate_may_be_provisional(db, zalsa, database_key_index, memo) {
            self.update_shallow(db, zalsa, database_key_index, memo, shallow_update);

            // SAFETY: memo is present in memo_map and we have verified that it is
            // still valid for the current revision.
            unsafe { Some(self.extend_memo_lifetime(memo)) }
        } else {
            None
        }
    }

    #[inline(never)]
    fn fetch_cold<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        db: &'db C::DbView,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C::Output<'db>>> {
        let database_key_index = self.database_key_index(id);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard = match zalsa.sync_table_for(id).claim(
            db,
            zalsa,
            database_key_index,
            memo_ingredient_index,
        ) {
            ClaimResult::Retry => return None,
            ClaimResult::Cycle => {
                // check if there's a provisional value for this query
                // Note we don't `validate_may_be_provisional` the memo here as we want to reuse an
                // existing provisional memo if it exists
                let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
                if let Some(memo) = memo_guard {
                    if memo.value.is_some()
                        && memo.revisions.cycle_heads.contains(&database_key_index)
                    {
                        if let Some(shallow_update) =
                            self.shallow_verify_memo(zalsa, database_key_index, memo)
                        {
                            self.update_shallow(
                                db,
                                zalsa,
                                database_key_index,
                                memo,
                                shallow_update,
                            );
                            // SAFETY: memo is present in memo_map.
                            return Some(unsafe { self.extend_memo_lifetime(memo) });
                        }
                    }
                }

                let initial_value = match C::CYCLE_STRATEGY {
                    CycleRecoveryStrategy::Fixpoint => {
                        C::cycle_initial(db, C::id_to_input(db, database_key_index.key_index()))
                    }
                    CycleRecoveryStrategy::FallbackImmediate => {
                        db.zalsa_local()
                            .assert_top_non_panic_cycle(database_key_index);
                        C::cycle_initial(db, C::id_to_input(db, database_key_index.key_index()))
                    }
                    CycleRecoveryStrategy::Panic => {
                        db.zalsa_local().cycle_panic(database_key_index, "querying")
                    }
                };

                tracing::debug!(
                    "hit cycle at {database_key_index:#?}, \
                    inserting and returning fixpoint initial value"
                );
                // no provisional value; create/insert/return initial provisional value
                return Some(self.insert_memo(
                    zalsa,
                    id,
                    Memo::new(
                        Some(initial_value),
                        zalsa.current_revision(),
                        QueryRevisions::fixpoint_initial(
                            database_key_index,
                            zalsa.current_revision(),
                        ),
                    ),
                    memo_ingredient_index,
                ));
            }
            ClaimResult::Claimed(guard) => guard,
        };

        let mut active_query = LazyActiveQueryGuard::new(database_key_index);

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
        if let Some(old_memo) = opt_old_memo {
            if old_memo.value.is_some() {
                if let VerifyResult::Unchanged(_, cycle_heads) =
                    self.deep_verify_memo(db, zalsa, old_memo, &mut active_query)
                {
                    if cycle_heads.is_empty() {
                        // SAFETY: memo is present in memo_map and we have verified that it is
                        // still valid for the current revision.
                        return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
                    }
                }
            }
        }

        let memo = self.execute(
            db,
            active_query.into_inner(db.zalsa_local(), C::CYCLE_STRATEGY),
            opt_old_memo,
        );

        Some(memo)
    }
}

pub(super) struct LazyActiveQueryGuard<'me> {
    guard: Option<ActiveQueryGuard<'me>>,
    database_key_index: DatabaseKeyIndex,
}

impl<'me> LazyActiveQueryGuard<'me> {
    pub(super) fn new(database_key_index: DatabaseKeyIndex) -> Self {
        Self {
            guard: None,
            database_key_index,
        }
    }

    pub(super) const fn database_key_index(&self) -> DatabaseKeyIndex {
        self.database_key_index
    }

    #[inline]
    pub(super) fn guard(
        &mut self,
        zalsa_local: &'me ZalsaLocal,
        cycle_strategy: CycleRecoveryStrategy,
    ) -> &ActiveQueryGuard<'me> {
        self.guard.get_or_insert_with(|| {
            zalsa_local.push_query(self.database_key_index, 0, cycle_strategy)
        })
    }

    #[inline]
    pub(super) fn into_inner(
        self,
        zalsa_local: &'me ZalsaLocal,
        cycle_strategy: CycleRecoveryStrategy,
    ) -> ActiveQueryGuard<'me> {
        self.guard
            .unwrap_or_else(|| zalsa_local.push_query(self.database_key_index, 0, cycle_strategy))
    }
}
