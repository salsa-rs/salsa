use crate::cycle::{CycleHeads, CycleRecoveryStrategy, IterationCount};
use crate::function::eviction::EvictionPolicy;
use crate::function::memo::Memo;
use crate::function::sync::ClaimResult;
use crate::function::{Configuration, IngredientImpl, Reentrancy};
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryRevisions, ZalsaLocal};
use crate::{DatabaseKeyIndex, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    #[inline]
    pub fn fetch<'db>(
        &'db self,
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        id: Id,
    ) -> &'db C::Output<'db> {
        zalsa.unwind_if_revision_cancelled(zalsa_local);

        let database_key_index = self.database_key_index(id);

        #[cfg(debug_assertions)]
        let _span = crate::tracing::debug_span!("fetch", query = ?database_key_index).entered();

        let memo = self.refresh_memo(db, zalsa, zalsa_local, id);

        // SAFETY: We just refreshed the memo so it is guaranteed to contain a value now.
        let memo_value = unsafe { memo.value.as_ref().unwrap_unchecked() };

        self.eviction.record_use(id);

        // Provisional reads depend on the active cycle heads for this iteration.
        if memo.may_be_provisional() {
            if let Some(memo_active_cycle) = memo.revisions.active_cycle() {
                if zalsa
                    .active_cycles()
                    .with_current_state_for_memo(
                        memo_active_cycle,
                        database_key_index,
                        |cycle_heads, transfer_cycle_heads| {
                            zalsa_local.report_tracked_read(
                                database_key_index,
                                memo.revisions.durability,
                                memo.revisions.changed_at,
                                (cycle_heads, transfer_cycle_heads, Some(memo_active_cycle)),
                                #[cfg(feature = "accumulator")]
                                memo.revisions.accumulated().is_some(),
                                #[cfg(feature = "accumulator")]
                                &memo.revisions.accumulated_inputs,
                            );
                        },
                    )
                    .is_some()
                {
                    return memo_value;
                }
            }
        }
        let empty_cycle_heads = CycleHeads::default();

        zalsa_local.report_tracked_read(
            database_key_index,
            memo.revisions.durability,
            memo.revisions.changed_at,
            (&empty_cycle_heads, &empty_cycle_heads, None),
            #[cfg(feature = "accumulator")]
            memo.revisions.accumulated().is_some(),
            #[cfg(feature = "accumulator")]
            &memo.revisions.accumulated_inputs,
        );

        memo_value
    }

    #[inline(always)]
    pub(super) fn refresh_memo<'db>(
        &'db self,
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        id: Id,
    ) -> &'db Memo<'db, C> {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

        loop {
            if let Some(memo) = self
                .fetch_hot(zalsa, id, memo_ingredient_index)
                .or_else(|| self.fetch_cold(zalsa, zalsa_local, db, id, memo_ingredient_index))
            {
                return memo;
            }
        }
    }

    #[inline(always)]
    fn fetch_hot<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
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
            if old_memo.value.is_some() {
                let can_shallow_update =
                    self.shallow_verify_memo(zalsa, database_key_index, old_memo);
                if can_shallow_update.yes()
                    && self.may_reuse_after_shallow_verify(zalsa, database_key_index, old_memo)
                {
                    self.update_shallow(zalsa, database_key_index, old_memo, can_shallow_update);

                    // SAFETY: memo is present in memo_map and we have verified that it is
                    // still valid for the current revision.
                    return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
                }

                if old_memo.was_cycle_participant() && zalsa_local.active_query().is_some() {
                    return self.execute(db, claim_guard, opt_old_memo);
                }

                let verify_result = self.deep_verify_memo(db, zalsa, old_memo, database_key_index);

                if verify_result.is_unchanged() {
                    // SAFETY: memo is present in memo_map and we have verified that it is
                    // still valid for the current revision.
                    return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
                }
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
                // check if there's a provisional value for this query
                // Note we don't check whether the memo is reusable here as we want to reuse an
                // existing provisional memo if it exists
                let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
                if let Some(memo) = &memo_guard {
                    if memo.verified_at.load() == zalsa.current_revision()
                        && memo.value.is_some()
                        && memo.revisions.active_cycle().is_some()
                    {
                        if let Some(memo_cycle) = memo.revisions.active_cycle() {
                            let active_cycle = zalsa.active_cycles().reuse_participant(
                                zalsa_local.active_cycle(),
                                memo_cycle,
                                database_key_index,
                            );
                            if let Some(active_cycle) = active_cycle {
                                memo.revisions.update_active_cycle(active_cycle);
                                crate::tracing::debug!(
                                    "hit cycle at {database_key_index:#?}, \
                                        returning last provisional value: {:#?}",
                                    memo.revisions
                                );

                                // SAFETY: memo is present in memo_map.
                                return unsafe { self.extend_memo_lifetime(memo) };
                            }
                        }
                    }
                }

                self.fetch_cold_cycle_initial(
                    zalsa,
                    zalsa_local,
                    db,
                    id,
                    database_key_index,
                    memo_ingredient_index,
                )
            }
        }
    }

    fn fetch_cold_cycle_initial<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        db: &'db C::DbView,
        id: Id,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> &'db Memo<'db, C> {
        crate::tracing::debug!(
            "hit cycle at {database_key_index:#?}, \
            inserting and returning fixpoint initial value"
        );

        let iteration = IterationCount::initial();
        let revisions = QueryRevisions::fixpoint_initial();
        let initial_value = C::cycle_initial(db, id, C::id_to_input(zalsa, id));
        let mut transfer_cycle_heads = CycleHeads::default();
        transfer_cycle_heads.insert(database_key_index);
        let active_cycle = if let Some(active_cycle) = zalsa_local.active_cycle() {
            zalsa
                .active_cycles()
                .add_head(active_cycle, database_key_index);
            active_cycle
        } else {
            zalsa.active_cycles().insert(database_key_index, iteration)
        };
        self.insert_memo(
            zalsa,
            id,
            Memo::new(Some(initial_value), zalsa.current_revision(), revisions).with_active_cycle(
                zalsa,
                database_key_index,
                active_cycle,
                &transfer_cycle_heads,
            ),
            memo_ingredient_index,
        )
    }
}
