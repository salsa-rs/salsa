use crate::cycle::{CycleHeads, CycleRecoveryStrategy};
use crate::function::memo::Memo;
use crate::function::sync::ClaimResult;
use crate::function::{Configuration, IngredientImpl, VerifyResult};
use crate::loom::sync::AtomicMut;
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::QueryRevisions;
use crate::Id;

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, id: Id) -> &'db C::Output<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        zalsa.unwind_if_revision_cancelled(zalsa_local);

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
                .fetch_hot(zalsa, id, memo_ingredient_index)
                .or_else(|| self.fetch_cold(zalsa, db, id, memo_ingredient_index))
            {
                // If we get back a provisional cycle memo, and it's provisional on any cycle heads
                // that are claimed by a different thread, we can't propagate the provisional memo
                // any further (it could escape outside the cycle); we need to block on the other
                // thread completing fixpoint iteration of the cycle, and then we can re-query for
                // our no-longer-provisional memo.
                // That is only correct for fixpoint cycles, though: `FallbackImmediate` cycles
                // never have provisional entries.
                if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate
                    || !memo.provisional_retry(zalsa, self.database_key_index(id))
                {
                    return memo;
                }
            }
        }
    }

    #[inline(always)]
    fn fetch_hot<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C::Output<'db>>> {
        let memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index)?;

        memo.value.as_ref()?;

        let database_key_index = self.database_key_index(id);

        let can_shallow_update = self.shallow_verify_memo(zalsa, database_key_index, memo);

        if can_shallow_update.yes() && !memo.may_be_provisional() {
            self.update_shallow(zalsa, database_key_index, memo, can_shallow_update);

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
        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard = match self.sync_table.try_claim(zalsa, id) {
            ClaimResult::Retry => return None,
            ClaimResult::Cycle => {
                let database_key_index = self.database_key_index(id);
                // check if there's a provisional value for this query
                // Note we don't `validate_may_be_provisional` the memo here as we want to reuse an
                // existing provisional memo if it exists
                let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
                if let Some(memo) = memo_guard {
                    if memo.value.is_some()
                        && memo.revisions.cycle_heads.contains(&database_key_index)
                    {
                        let can_shallow_update =
                            self.shallow_verify_memo(zalsa, database_key_index, memo);
                        if can_shallow_update.yes() {
                            self.update_shallow(
                                zalsa,
                                database_key_index,
                                memo,
                                can_shallow_update,
                            );
                            // SAFETY: memo is present in memo_map.
                            return unsafe { Some(self.extend_memo_lifetime(memo)) };
                        }
                    }
                }
                // no provisional value; create/insert/return initial provisional value
                return match C::CYCLE_STRATEGY {
                    CycleRecoveryStrategy::Panic => db.zalsa_local().with_query_stack(|stack| {
                        panic!(
                            "dependency graph cycle when querying {database_key_index:#?}, \
                            set cycle_fn/cycle_initial to fixpoint iterate.\n\
                            Query stack:\n{stack:#?}",
                        );
                    }),
                    CycleRecoveryStrategy::Fixpoint => {
                        tracing::debug!(
                            "hit cycle at {database_key_index:#?}, \
                            inserting and returning fixpoint initial value"
                        );
                        let revisions = QueryRevisions::fixpoint_initial(database_key_index);
                        let initial_value = self
                            .initial_value(db, id)
                            .expect("`CycleRecoveryStrategy::Fixpoint` should have initial_value");
                        Some(self.insert_memo(
                            zalsa,
                            id,
                            Memo::new(Some(initial_value), zalsa.current_revision(), revisions),
                            memo_ingredient_index,
                        ))
                    }
                    CycleRecoveryStrategy::FallbackImmediate => {
                        tracing::debug!(
                            "hit a `FallbackImmediate` cycle at {database_key_index:#?}"
                        );
                        let active_query = db.zalsa_local().push_query(database_key_index, 0);
                        let fallback_value = self.initial_value(db, id).expect(
                            "`CycleRecoveryStrategy::FallbackImmediate` should have initial_value",
                        );
                        let mut revisions = active_query.pop();
                        revisions.cycle_heads = CycleHeads::initial(database_key_index);
                        // We need this for `cycle_heads()` to work. We will unset this in the outer `execute()`.
                        revisions.verified_final.write_mut(false);
                        Some(self.insert_memo(
                            zalsa,
                            id,
                            Memo::new(Some(fallback_value), zalsa.current_revision(), revisions),
                            memo_ingredient_index,
                        ))
                    }
                };
            }
            ClaimResult::Claimed(guard) => guard,
        };

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
        if let Some(old_memo) = opt_old_memo {
            if old_memo.value.is_some() {
                if let VerifyResult::Unchanged(_, cycle_heads) =
                    self.deep_verify_memo(db, zalsa, old_memo, self.database_key_index(id))
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
            db.zalsa_local().push_query(self.database_key_index(id), 0),
            opt_old_memo,
        );

        Some(memo)
    }
}
