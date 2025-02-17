use super::{memo::Memo, Configuration, IngredientImpl, VerifyResult};
use crate::{
    accumulator::accumulated_map::InputAccumulatedValues, runtime::StampedValue,
    table::sync::ClaimResult, zalsa::ZalsaDatabase, zalsa_local::QueryRevisions,
    AsDynDatabase as _, Id,
};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn fetch<'db>(&'db self, db: &'db C::DbView, id: Id) -> &'db C::Output<'db> {
        let zalsa_local = db.zalsa_local();
        zalsa_local.unwind_if_revision_cancelled(db.as_dyn_database());

        let memo = self.refresh_memo(db, id);
        let StampedValue {
            value,
            durability,
            changed_at,
        } = memo.revisions.stamped_value(memo.value.as_ref().unwrap());

        self.lru.record_use(id);

        zalsa_local.report_tracked_read(
            self.database_key_index(id).into(),
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
        loop {
            if let Some(memo) = self.fetch_hot(db, id).or_else(|| self.fetch_cold(db, id)) {
                // If we get back a provisional cycle memo, and it's provisional on any cycle heads
                // that are claimed by a different thread, we can't propagate the provisional memo
                // any further (it could escape outside the cycle); we need to block on the other
                // thread completing fixpoint iteration of the cycle, and then we can re-query for
                // our no-longer-provisional memo.
                if !(memo.may_be_provisional()
                    && memo.provisional_retry(db.as_dyn_database(), self.database_key_index(id)))
                {
                    return memo;
                }
            }
        }
    }

    #[inline]
    fn fetch_hot<'db>(&'db self, db: &'db C::DbView, id: Id) -> Option<&'db Memo<C::Output<'db>>> {
        let zalsa = db.zalsa();
        let memo_guard = self.get_memo_from_table_for(zalsa, id);
        if let Some(memo) = &memo_guard {
            if memo.value.is_some()
                && self.shallow_verify_memo(db, zalsa, self.database_key_index(id), memo, false)
            {
                // Unsafety invariant: memo is present in memo_map and we have verified that it is
                // still valid for the current revision.
                return unsafe { Some(self.extend_memo_lifetime(memo)) };
            }
        }
        None
    }

    fn fetch_cold<'db>(&'db self, db: &'db C::DbView, id: Id) -> Option<&'db Memo<C::Output<'db>>> {
        let (zalsa, zalsa_local) = db.zalsas();
        let database_key_index = self.database_key_index(id);

        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let _claim_guard = match zalsa.sync_table_for(id).claim(
            db.as_dyn_database(),
            zalsa_local,
            database_key_index,
            self.memo_ingredient_index,
        ) {
            ClaimResult::Retry => return None,
            ClaimResult::Cycle => {
                // check if there's a provisional value for this query
                let memo_guard = self.get_memo_from_table_for(zalsa, id);
                if let Some(memo) = &memo_guard {
                    if memo.value.is_some()
                        && memo.revisions.cycle_heads.contains(&database_key_index)
                        && self.shallow_verify_memo(db, zalsa, database_key_index, memo, true)
                    {
                        // Unsafety invariant: memo is present in memo_map.
                        unsafe {
                            return Some(self.extend_memo_lifetime(memo));
                        }
                    }
                }
                // no provisional value; create/insert/return initial provisional value
                return self
                    .initial_value(db, database_key_index.key_index)
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

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        let active_query = zalsa_local.push_query(database_key_index);
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id);
        if let Some(old_memo) = &opt_old_memo {
            if old_memo.value.is_some() {
                if let VerifyResult::Unchanged(_, cycle_heads) =
                    self.deep_verify_memo(db, old_memo, &active_query)
                {
                    if cycle_heads.is_empty() {
                        // Unsafety invariant: memo is present in memo_map and we have verified that it is
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
