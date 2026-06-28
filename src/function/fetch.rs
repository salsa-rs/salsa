use crate::cycle::IterationStamp;
use crate::function::cycle_strategy::{CycleStrategy, FetchCycleContext};
use crate::function::eviction::EvictionPolicy;
use crate::function::execute::CycleState;
use crate::function::memo::{ErasedMemo, Memo};
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

        #[cfg(feature = "detailed-trace")]
        let _span = crate::tracing::debug_span!("fetch", query = ?database_key_index).entered();

        let memo = self.refresh_memo(db, zalsa, zalsa_local, id);

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
    pub(super) fn refresh_memo<'db>(
        &'db self,
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        id: Id,
    ) -> &'db Memo<'db, C> {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

        loop {
            // Keep the hot and cold probes in distinct control-flow blocks. Using `or_else`
            // here can outline both into one function, making hot hits pay for the cold path's
            // stack frame.
            if let Some(memo) = self.fetch_hot(zalsa, id, memo_ingredient_index) {
                return memo;
            }

            if let Some(memo) = self.fetch_cold(zalsa, zalsa_local, db, id, memo_ingredient_index) {
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

        let can_shallow_update = memo.header.shallow_verify_memo(
            zalsa,
            database_key_index,
            #[cfg(feature = "detailed-trace")]
            true,
        );

        if can_shallow_update.yes() && !memo.header.may_be_provisional() {
            memo.header
                .update_shallow(zalsa, database_key_index, can_shallow_update);

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
                    db,
                    zalsa,
                    zalsa_local,
                    database_key_index,
                    memo_ingredient_index,
                ));
            }
        };

        // Now that we've claimed the item, check again to see if there's a hot value.
        let opt_old_memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);

        if let Some(old_memo) = opt_old_memo {
            if old_memo.value.is_some()
                && old_memo.header.verify_memo(
                    db.into(),
                    &claim_guard,
                    C::CYCLE_RECOVERY_STRATEGY,
                    #[cfg(feature = "detailed-trace")]
                    true,
                )
            {
                // SAFETY: The memo is present in the memo table, and we verified that it is valid
                // for the current revision.
                return unsafe { Some(self.extend_memo_lifetime(old_memo)) };
            }
        }

        self.execute(db, claim_guard, opt_old_memo, memo_ingredient_index)
    }

    #[cold]
    fn fetch_cold_cycle<'db>(
        &'db self,
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> &'db Memo<'db, C> {
        <C::CycleStrategy as CycleStrategy<C>>::fetch_cold_cycle(FetchCycleContext {
            ingredient: self,
            db,
            zalsa,
            zalsa_local,
            database_key_index,
            memo_ingredient_index,
        })
    }
}

pub(super) fn fetch_cold_cycle_panic(
    zalsa_local: &ZalsaLocal,
    database_key_index: DatabaseKeyIndex,
) -> ! {
    // SAFETY: We do not access the query stack reentrantly.
    unsafe {
        zalsa_local.with_query_stack_unchecked(|stack| {
            panic!(
                "dependency graph cycle when querying {database_key_index:#?}, \
                set cycle_fn/cycle_initial to fixpoint iterate.\n\
                Query stack:\n{stack:#?}",
            );
        })
    }
}

pub(super) fn fetch_cold_cycle_recoverable_erased<'db>(
    state: &mut dyn CycleState<'db>,
    zalsa: &'db Zalsa,
    database_key_index: DatabaseKeyIndex,
) -> ErasedMemo<'db> {
    let id = database_key_index.key_index();

    let cancellation_count = zalsa.runtime().cancellation_count();
    // Don't validate provisional memos here: an existing value should be reused.
    let current_memo = state.provisional_memo(zalsa, id).filter(|memo| {
        let header = memo.header();
        header.verified_at.load() == zalsa.current_revision()
            && memo.has_value()
            && header.revisions.iteration().cancellation_count() == cancellation_count
    });

    // Ideally, any current provisional value could be reused. Reusing a value that was not a
    // cycle head in the last iteration would require inserting itself as a head, which in turn
    // requires cloning the value or making the cycle-head list concurrent.
    if let Some(memo) = current_memo.filter(|memo| {
        memo.header()
            .revisions
            .cycle_heads()
            .contains(&database_key_index)
    }) {
        memo.header()
            .revisions
            .cycle_heads()
            .remove_all_except(database_key_index);

        crate::tracing::debug!(
            "hit cycle at {database_key_index:#?}, \
                    returning last provisional value: {:#?}",
            memo.header().revisions
        );
        return memo;
    }

    crate::tracing::debug!(
        "hit cycle at {database_key_index:#?}, \
        inserting and returning fixpoint initial value"
    );

    let iteration = current_memo
        .map(|memo| memo.header().revisions.iteration())
        .unwrap_or_else(|| IterationStamp::initial(cancellation_count));
    let revisions = QueryRevisions::fixpoint_initial(database_key_index, iteration);
    state.use_fallback(zalsa, id);
    state.insert_provisional_memo(zalsa, id, revisions)
}
