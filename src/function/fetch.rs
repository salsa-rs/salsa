use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl, VerifyResult};
use crate::runtime::StampedValue;
use crate::table::sync::ClaimResult;
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::QueryRevisions;
use crate::{AsDynDatabase as _, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, id: Id) -> &'db C::Output<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        zalsa.unwind_if_revision_cancelled(db);

        let memo = self.refresh_memo(db, id);
        // SAFETY: We just refreshed the memo so it is guaranteed to contain a value now.
        let memo_value = unsafe { memo.value.as_ref().unwrap_unchecked() };
        let StampedValue {
            value,
            durability,
            changed_at,
        } = memo.revisions.stamped_value(memo_value);

        self.lru.record_use(id);

        zalsa_local.report_tracked_read(
            self.database_key_index(id),
            durability,
            changed_at,
            match &memo.revisions.accumulated {
                Some(_) => InputAccumulatedValues::Any,
                None => memo.revisions.accumulated_inputs.load(),
            },
            memo.cycle_heads(),
        );

        value
    }

    #[inline]
    pub(super) fn refresh_memo<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
    ) -> &'db Memo<C::Output<'db>> {
        let zalsa = db.zalsa();
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
                if !(memo.may_be_provisional()
                    && memo.provisional_retry(
                        db.as_dyn_database(),
                        zalsa,
                        self.database_key_index(id),
                    ))
                {
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
        let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
        if let Some(memo) = memo_guard {
            let database_key_index = self.database_key_index(id);
            if memo.value.is_some()
                && (self.validate_may_be_provisional(db, zalsa, database_key_index, memo)
                    || self.validate_same_iteration(db, database_key_index, memo))
                && self.shallow_verify_memo(db, zalsa, database_key_index, memo)
            {
                // SAFETY: memo is present in memo_map and we have verified that it is
                // still valid for the current revision.
                return unsafe { Some(self.extend_memo_lifetime(memo)) };
            }
        }
        None
    }

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
                        && self.shallow_verify_memo(db, zalsa, database_key_index, memo)
                    {
                        // SAFETY: memo is present in memo_map.
                        return unsafe { Some(self.extend_memo_lifetime(memo)) };
                    }
                }
                // no provisional value; create/insert/return initial provisional value
                return self
                    .initial_value(db, database_key_index.key_index())
                    .map(|initial_value| {
                        tracing::debug!(
                            "hit cycle at {database_key_index:#?}, \
                            inserting and returning fixpoint initial value"
                        );
                        self.insert_memo(
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
                        )
                    })
                    .or_else(|| {
                        panic!(
                            "dependency graph cycle querying {database_key_index:#?}; \
                             set cycle_fn/cycle_initial to fixpoint iterate"
                        )
                    });
            }
            ClaimResult::Claimed(guard) => guard,
        };

        // Push the query on the stack.
        let active_query = db.zalsa_local().push_query(database_key_index, 0);

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
        if let Some(old_memo) = opt_old_memo {
            if old_memo.value.is_some() {
                if let VerifyResult::Unchanged(_, cycle_heads) =
                    self.deep_verify_memo(db, zalsa, old_memo, &active_query)
                {
                    if cycle_heads.is_empty() {
                        // SAFETY: memo is present in memo_map and we have verified that it is
                        // still valid for the current revision.
                        return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
                    }
                }
            }
        }

        let memo = self.execute(db, active_query, opt_old_memo);

        Some(memo)
    }
}
