use rustc_hash::FxHashMap;

use crate::cycle::{CycleHeads, CycleRecoveryStrategy, IterationCount};
use crate::function::maybe_changed_after::VerifyCycleHeads;
use crate::function::memo::Memo;
use crate::function::sync::ClaimResult;
use crate::function::{Configuration, IngredientImpl};
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryRevisions, ZalsaLocal};
use crate::{DatabaseKeyIndex, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
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

        self.lru.record_use(id);

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
                .or_else(|| {
                    self.fetch_cold_with_retry(zalsa, zalsa_local, db, id, memo_ingredient_index)
                })
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

    #[inline(never)]
    fn fetch_cold_with_retry<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        db: &'db C::DbView,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<'db, C>> {
        let memo = self.fetch_cold(zalsa, zalsa_local, db, id, memo_ingredient_index)?;

        // If we get back a provisional cycle memo, and it's provisional on any cycle heads
        // that are claimed by a different thread, we can't propagate the provisional memo
        // any further (it could escape outside the cycle); we need to block on the other
        // thread completing fixpoint iteration of the cycle, and then we can re-query for
        // our no-longer-provisional memo.
        // That is only correct for fixpoint cycles, though: `FallbackImmediate` cycles
        // never have provisional entries.
        if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate
            || !memo.provisional_retry(zalsa, zalsa_local, self.database_key_index(id))
        {
            Some(memo)
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
        let claim_guard = match self.sync_table.try_claim(zalsa, id) {
            ClaimResult::Claimed(guard) => guard,
            ClaimResult::Running(blocked_on) => {
                blocked_on.block_on(zalsa);

                let memo = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);

                if let Some(memo) = memo {
                    // This isn't strictly necessary, but if this is a provisional memo for an inner cycle,
                    // await all outer cycle heads to give the thread driving it a chance to complete
                    // (we don't want multiple threads competing for the queries participating in the same cycle).
                    if memo.value.is_some() && memo.may_be_provisional() {
                        memo.block_on_heads(zalsa, zalsa_local);
                    }
                }
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

                // If this is a provisional memo from the same revision, await all its cycle heads because
                // we need to ensure that only one thread is iterating on a cycle at a given time.
                // For example, if we have a nested cycle like so:
                // ```
                // a -> b -> c -> b
                //        -> a
                //
                // d -> b
                // ```
                // thread 1 calls `a` and `a` completes the inner cycle `b -> c` but hasn't finished the outer cycle `a` yet.
                // thread 2 now calls `b`. We don't want that thread 2 iterates `b` while thread 1 is iterating `a` at the same time
                // because it can result in thread b overriding provisional memos that thread a has accessed already and still relies upon.
                //
                // By waiting, we ensure that thread 1 completes a (based on a provisional value for `b`) and `b`
                // becomes the new outer cycle, which thread 2 drives to completion.
                if old_memo.may_be_provisional()
                    && old_memo.verified_at.load() == zalsa.current_revision()
                {
                    // Try to claim all cycle heads of the provisional memo. If we can't because
                    // some head is running on another thread, drop our claim guard to give that thread
                    // a chance to take ownership of this query and complete it as part of its fixpoint iteration.
                    // We will then block on the cycle head and retry once all cycle heads completed.
                    if !old_memo.try_claim_heads(zalsa, zalsa_local) {
                        drop(claim_guard);
                        old_memo.block_on_heads(zalsa, zalsa_local);
                        return None;
                    }
                }
            }
        }

        let memo = self.execute(db, zalsa, zalsa_local, database_key_index, opt_old_memo);

        Some(memo)
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
        // check if there's a provisional value for this query
        // Note we don't `validate_may_be_provisional` the memo here as we want to reuse an
        // existing provisional memo if it exists
        let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
        if let Some(memo) = memo_guard {
            if memo.value.is_some() && memo.revisions.cycle_heads().contains(&database_key_index) {
                let can_shallow_update = self.shallow_verify_memo(zalsa, database_key_index, memo);
                if can_shallow_update.yes() {
                    self.update_shallow(zalsa, database_key_index, memo, can_shallow_update);
                    // SAFETY: memo is present in memo_map.
                    return unsafe { self.extend_memo_lifetime(memo) };
                }
            }
        }

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
            CycleRecoveryStrategy::Fixpoint => {
                crate::tracing::debug!(
                    "hit cycle at {database_key_index:#?}, \
                    inserting and returning fixpoint initial value"
                );
                let revisions = QueryRevisions::fixpoint_initial(database_key_index);
                let initial_value = C::cycle_initial(db, C::id_to_input(zalsa, id));
                self.insert_memo(
                    zalsa,
                    id,
                    Memo::new(Some(initial_value), zalsa.current_revision(), revisions),
                    memo_ingredient_index,
                )
            }
            CycleRecoveryStrategy::FallbackImmediate => {
                crate::tracing::debug!(
                    "hit a `FallbackImmediate` cycle at {database_key_index:#?}"
                );
                let active_query =
                    zalsa_local.push_query(database_key_index, IterationCount::initial());
                let fallback_value = C::cycle_initial(db, C::id_to_input(zalsa, id));
                let mut completed_query = active_query.pop();
                completed_query
                    .revisions
                    .set_cycle_heads(CycleHeads::initial(database_key_index));
                // We need this for `cycle_heads()` to work. We will unset this in the outer `execute()`.
                *completed_query.revisions.verified_final.get_mut() = false;
                self.insert_memo(
                    zalsa,
                    id,
                    Memo::new(
                        Some(fallback_value),
                        zalsa.current_revision(),
                        completed_query.revisions,
                    ),
                    memo_ingredient_index,
                )
            }
        }
    }
}
