use rustc_hash::FxHashMap;

use crate::cycle::{CycleRecoveryStrategy, IterationCount};
use crate::function::eviction::EvictionPolicy;
use crate::function::maybe_changed_after::VerifyCycleHeads;
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

        zalsa_local.report_tracked_read(
            database_key_index,
            memo.revisions.durability,
            memo.revisions.changed_at,
            memo.cycle_heads(),
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
        let claim_guard = match self.sync_table.try_claim(zalsa, id, Reentrancy::Allow) {
            ClaimResult::Claimed(guard) => guard,
            ClaimResult::Running(blocked_on) => {
                blocked_on.block_on(zalsa);
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
                    && self.validate_may_be_provisional(
                        zalsa,
                        zalsa_local,
                        database_key_index,
                        old_memo,
                    )
                {
                    self.update_shallow(zalsa, database_key_index, old_memo, can_shallow_update);

                    // SAFETY: memo is present in memo_map and we have verified that it is
                    // still valid for the current revision.
                    return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
                }

                let mut cycle_heads = Vec::new();
                let mut participating_queries = FxHashMap::default();

                let verify_result = self.deep_verify_memo(
                    db,
                    zalsa,
                    old_memo,
                    database_key_index,
                    &mut VerifyCycleHeads::new(&mut cycle_heads, &mut participating_queries),
                    can_shallow_update,
                );

                if verify_result.is_unchanged() && cycle_heads.is_empty() {
                    // SAFETY: memo is present in memo_map and we have verified that it is
                    // still valid for the current revision.
                    return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
                }
            }
        }

        self.execute(db, claim_guard, zalsa_local, opt_old_memo)
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
                // Note we don't `validate_may_be_provisional` the memo here as we want to reuse an
                // existing provisional memo if it exists
                let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
                if let Some(memo) = &memo_guard {
                    // Ideally, we'd use the last provisional memo even if it wasn't a cycle head in the last iteration
                    // but that would require inserting itself as a cycle head, which either requires clone
                    // on the value OR a concurrent `Vec` for cycle heads.
                    if memo.verified_at.load() == zalsa.current_revision()
                        && memo.value.is_some()
                        && memo.revisions.cycle_heads().contains(&database_key_index)
                    {
                        memo.revisions
                            .cycle_heads()
                            .remove_all_except(database_key_index);

                        crate::tracing::debug!(
                            "hit cycle at {database_key_index:#?}, \
                                returning last provisional value: {:#?}",
                            memo.revisions
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
                        if old_memo.verified_at.load() == zalsa.current_revision()
                            && old_memo.value.is_some()
                        {
                            Some(old_memo.revisions.iteration())
                        } else {
                            None
                        }
                    })
                    .unwrap_or(IterationCount::initial());
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
