use crate::cycle::{CycleRecoveryStrategy, IterationStamp};
use crate::function::eviction::EvictionPolicy;
use crate::function::memo::Memo;
use crate::function::sync::ClaimResult;
use crate::function::{Configuration, IngredientImpl, IngredientInDb, Reentrancy};
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryRevisions, ZalsaLocal};
use crate::{DatabaseKeyIndex, Id};

impl<'db, C> IngredientInDb<'db, C>
where
    C: Configuration,
{
    #[inline]
    pub fn fetch(&self, id: Id) -> &'db C::Output<'db> {
        let zalsa = self.zalsa;
        let zalsa_local = self.zalsa_local;

        zalsa.unwind_if_revision_cancelled(zalsa_local);

        let database_key_index = self.database_key_index(id);

        #[cfg(feature = "detailed-trace")]
        let _span = crate::tracing::debug_span!("fetch", query = ?database_key_index).entered();

        let memo = self.refresh_memo(id);

        // SAFETY: We just refreshed the memo so it is guaranteed to contain a value now.
        let memo_value = unsafe { memo.value.as_ref().unwrap_unchecked() };

        self.eviction.record_use(id);

        let revisions = &memo.header.revisions;
        zalsa_local.report_tracked_read(
            database_key_index,
            revisions.durability,
            revisions.changed_at,
            memo.header.cycle_heads(),
            #[cfg(feature = "accumulator")]
            revisions.accumulated().is_some(),
            #[cfg(feature = "accumulator")]
            &revisions.accumulated_inputs,
        );

        memo_value
    }

    #[inline(always)]
    pub(super) fn refresh_memo(&self, id: Id) -> &'db Memo<'db, C> {
        let ingredient = self.ingredient;
        let zalsa = self.zalsa;
        let memo_ingredient_index = ingredient.memo_ingredient_index(zalsa, id);

        loop {
            // Keep the hot and cold probes in distinct control-flow blocks. Using `or_else`
            // here can outline both into one function, making hot hits pay for the cold path's
            // stack frame.
            if let Some(memo) = self.fetch_hot(id, memo_ingredient_index) {
                return memo;
            }

            if let Some(memo) = self.fetch_cold(id, memo_ingredient_index) {
                return memo;
            }
        }
    }

    #[inline(always)]
    fn fetch_hot(
        &self,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        let ingredient = self.ingredient;
        let zalsa = self.zalsa;

        // SAFETY: `IngredientInDb` guarantees that the ingredient is registered in `zalsa`, and
        // `memo_ingredient_index` was read from that ingredient's memo map.
        let memo = unsafe {
            ingredient.get_memo_from_table_for_unchecked(zalsa, id, memo_ingredient_index)?
        };

        memo.value.as_ref()?;

        let database_key_index = self.database_key_index(id);

        let can_shallow_update = memo
            .header
            .shallow_verify_memo(zalsa, database_key_index, true);

        if can_shallow_update.yes() && !memo.header.may_be_provisional() {
            memo.header
                .update_shallow(zalsa, database_key_index, can_shallow_update);

            // SAFETY: memo is present in memo_map and we have verified that it is
            // still valid for the current revision.
            unsafe { Some(ingredient.extend_memo_lifetime(memo)) }
        } else {
            None
        }
    }

    #[inline(always)]
    fn fetch_cold(
        &self,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        self.ingredient.fetch_cold(
            self.zalsa,
            self.zalsa_local,
            self.db,
            id,
            memo_ingredient_index,
        )
    }
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    fn fetch_cold<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        db: &'db C::DbView,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        let database_key_index = self.database_key_index(id);
        // Try to claim this query: if someone else has claimed it already, go back and start again.
        let claim_guard = match self
            .sync_table
            .try_claim(zalsa, zalsa_local, id, Reentrancy::Allow)
        {
            ClaimResult::Claimed(guard) => guard,
            ClaimResult::Running(blocked_on) => {
                let _ = blocked_on.block_on(zalsa);
                return None;
            }
            ClaimResult::Cycle { .. } => {
                return Some(self.fetch_cold_cycle(
                    zalsa,
                    zalsa_local,
                    db,
                    id,
                    database_key_index,
                    memo_ingredient_index,
                ));
            }
        };

        // Now that we've claimed the item, check again to see if there's a "hot" value.
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);

        if let Some(old_memo) = opt_old_memo {
            if old_memo.value.is_some()
                && old_memo
                    .header
                    .verify_memo(db.into(), &claim_guard, C::CYCLE_STRATEGY, true)
            {
                // SAFETY: memo is present in memo_map and we have verified that it is
                // still valid for the current revision.
                return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
            }
        }

        self.execute(db, claim_guard, opt_old_memo)
    }

    #[cold]
    #[inline(never)]
    fn fetch_cold_cycle<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        db: &'db C::DbView,
        id: Id,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> &'db Memo<'db, C> {
        // no provisional value; create/insert/return initial provisional value
        match C::CYCLE_STRATEGY {
            // SAFETY: We do not access the query stack reentrantly.
            CycleRecoveryStrategy::Panic => unsafe {
                zalsa_local.with_query_stack_unchecked(|stack| {
                    panic!(
                        "dependency graph cycle when querying {database_key_index:#?}, \
                    set cycle_fn/cycle_initial to fixpoint iterate.\n\
                    Query stack:\n{stack:#?}",
                    );
                })
            },
            CycleRecoveryStrategy::Fixpoint | CycleRecoveryStrategy::FallbackImmediate => {
                let cancellation_count = zalsa.runtime().cancellation_count();
                // check if there's a provisional value for this query
                // Note we don't `validate_may_be_provisional` the memo here as we want to reuse an
                // existing provisional memo if it exists
                let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
                if let Some(memo) = &memo_guard {
                    let revisions = &memo.header.revisions;
                    // Ideally, we'd use the last provisional memo even if it wasn't a cycle head in the last iteration
                    // but that would require inserting itself as a cycle head, which either requires clone
                    // on the value OR a concurrent `Vec` for cycle heads.
                    if memo.header.verified_at.load() == zalsa.current_revision()
                        && memo.value.is_some()
                        && revisions.iteration().cancellation_count() == cancellation_count
                        && revisions.cycle_heads().contains(&database_key_index)
                    {
                        revisions
                            .cycle_heads()
                            .remove_all_except(database_key_index);

                        crate::tracing::debug!(
                            "hit cycle at {database_key_index:#?}, \
                                returning last provisional value: {:#?}",
                            revisions
                        );

                        // SAFETY: memo is present in memo_map.
                        return unsafe { self.extend_memo_lifetime(memo) };
                    }
                }

                crate::tracing::debug!(
                    "hit cycle at {database_key_index:#?}, \
                    inserting and returning fixpoint initial value"
                );

                let iteration = memo_guard
                    .and_then(|old_memo| {
                        let revisions = &old_memo.header.revisions;
                        if old_memo.header.verified_at.load() == zalsa.current_revision()
                            && old_memo.value.is_some()
                            && revisions.iteration().cancellation_count() == cancellation_count
                        {
                            Some(revisions.iteration())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| IterationStamp::initial(cancellation_count));
                let revisions = QueryRevisions::fixpoint_initial(database_key_index, iteration);

                let initial_value = C::cycle_initial(db, id, C::id_to_input(zalsa, id));
                self.insert_memo(
                    zalsa,
                    id,
                    Memo::new(Some(initial_value), zalsa.current_revision(), revisions),
                    memo_ingredient_index,
                )
            }
        }
    }
}
