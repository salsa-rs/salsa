#[cfg(feature = "accumulator")]
use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::cycle::{CycleHeads, CycleRecoveryStrategy, ProvisionalStatus};
use crate::database::RawDatabase;
use crate::function::memo::{
    ErasedMemo, FunctionMemoTable, MemoHeader, TryClaimCycleHeadsIter, TryClaimHeadsResult,
};
use crate::function::sync::{ClaimGuard, ClaimResult};
use crate::function::{Configuration, IngredientImpl, Reentrancy, SyncTable};
use std::sync::atomic::Ordering;

use crate::key::DatabaseKeyIndex;
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::{QueryEdgeKind, QueryEdges, QueryOriginRef, QueryRevisions, ZalsaLocal};
use crate::{Id, Revision};

/// Result of memo validation.
#[derive(Debug, Copy, Clone)]
pub enum VerifyResult {
    /// Memo has changed and needs to be recomputed.
    Changed,

    /// Memo remains valid.
    ///
    /// The inner value tracks whether the memo or any of its dependencies have an
    /// accumulated value.
    Unchanged {
        #[cfg(feature = "accumulator")]
        accumulated: InputAccumulatedValues,
    },
}

impl VerifyResult {
    pub(crate) const fn changed_if(changed: bool) -> Self {
        if changed {
            Self::changed()
        } else {
            Self::unchanged()
        }
    }

    pub(crate) const fn changed() -> Self {
        Self::Changed
    }

    pub(crate) const fn unchanged() -> Self {
        Self::Unchanged {
            #[cfg(feature = "accumulator")]
            accumulated: InputAccumulatedValues::Empty,
        }
    }

    #[inline]
    #[cfg(feature = "accumulator")]
    pub(crate) fn unchanged_with_accumulated(accumulated: InputAccumulatedValues) -> Self {
        Self::Unchanged { accumulated }
    }

    #[inline]
    #[cfg(not(feature = "accumulator"))]
    pub(crate) fn unchanged_with_accumulated() -> Self {
        Self::unchanged()
    }

    /// Returns an unchanged result that propagates accumulated values from both
    /// the memo itself and its inputs.
    #[inline]
    fn unchanged_for_memo(revisions: &QueryRevisions) -> Self {
        #[cfg(not(feature = "accumulator"))]
        let _ = revisions;

        Self::unchanged_with_accumulated(
            #[cfg(feature = "accumulator")]
            match revisions.accumulated() {
                Some(_) => InputAccumulatedValues::Any,
                None => revisions.accumulated_inputs.load(),
            },
        )
    }

    pub(crate) const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged { .. })
    }
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn maybe_changed_after<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
        revision: Revision,
    ) -> VerifyResult {
        let (zalsa, zalsa_local) = db.zalsas();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);
        zalsa.unwind_if_revision_cancelled(zalsa_local);

        loop {
            let database_key_index = self.database_key_index(id);

            crate::tracing::debug!(
                "{database_key_index:?}: maybe_changed_after(revision = {revision:?})"
            );

            // Check if we have a verified version: this is the hot path.
            let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
            let Some(memo) = memo_guard else {
                // No memo? Assume has changed.
                return VerifyResult::changed();
            };

            if let Some(result) = memo.header.maybe_changed_after_hot(
                zalsa,
                database_key_index,
                revision,
                #[cfg(feature = "detailed-trace")]
                memo.value.is_some(),
            ) {
                return result;
            }

            if let Some(result) = self.maybe_changed_after_cold(
                zalsa,
                zalsa_local,
                db,
                database_key_index,
                revision,
                memo_ingredient_index,
            ) {
                return result;
            }
        }
    }

    fn maybe_changed_after_cold(
        &self,
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        db: &C::DbView,
        database_key_index: DatabaseKeyIndex,
        revision: Revision,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<VerifyResult> {
        enum ColdResult<'db> {
            Retry,
            Verified(VerifyResult),
            Reexecute {
                claim_guard: ClaimGuard<'db>,
                old_memo: ErasedMemo<'db>,
            },
        }

        #[allow(clippy::too_many_arguments)]
        fn inner<'db>(
            sync_table: &'db SyncTable,
            zalsa: &'db Zalsa,
            zalsa_local: &'db ZalsaLocal,
            db: RawDatabase<'db>,
            memo_table: FunctionMemoTable<'db>,
            database_key_index: DatabaseKeyIndex,
            revision: Revision,
            memo_ingredient_index: MemoIngredientIndex,
            cycle_recovery_strategy: CycleRecoveryStrategy,
        ) -> ColdResult<'db> {
            let claim_guard = match sync_table.try_claim(
                zalsa,
                zalsa_local,
                database_key_index.key_index(),
                Reentrancy::Deny,
            ) {
                ClaimResult::Claimed(guard) => guard,
                ClaimResult::Running(blocked_on) => {
                    let _ = blocked_on.block_on(zalsa);
                    return ColdResult::Retry;
                }
                ClaimResult::Cycle { .. } => {
                    return ColdResult::Verified(maybe_changed_after_cold_cycle(
                        zalsa_local,
                        database_key_index,
                        cycle_recovery_strategy,
                    ));
                }
            };

            // Load the current memo after claiming the query because it may have changed while
            // this query was blocked on another thread.
            let Some(old_memo) = memo_table.get_erased(memo_ingredient_index) else {
                return ColdResult::Verified(VerifyResult::changed());
            };

            let old_header = old_memo.header();

            crate::tracing::debug!(
                "{database_key_index:?}: maybe_changed_after_cold, successful claim, \
                    revision = {revision:?}, old_memo = {old_memo:#?}",
                old_memo = old_header.tracing_debug(old_memo.has_value()),
            );

            if old_header.verify_memo(
                db,
                &claim_guard,
                cycle_recovery_strategy,
                #[cfg(feature = "detailed-trace")]
                old_memo.has_value(),
            ) {
                return ColdResult::Verified(if old_header.revisions.changed_at > revision {
                    VerifyResult::changed()
                } else {
                    VerifyResult::unchanged_for_memo(&old_header.revisions)
                });
            }

            // If the memo is not provisional, the generic continuation can check whether it has
            // an old value and re-execute. The result may equal the old value and be backdated, in
            // which case the new memo has not logically changed.
            if !old_header.may_be_provisional() {
                ColdResult::Reexecute {
                    claim_guard,
                    old_memo,
                }
            } else {
                ColdResult::Verified(VerifyResult::changed())
            }
        }

        match inner(
            &self.sync_table,
            zalsa,
            zalsa_local,
            db.into(),
            self.memo_table_for(zalsa, database_key_index.key_index()),
            database_key_index,
            revision,
            memo_ingredient_index,
            C::CYCLE_STRATEGY,
        ) {
            ColdResult::Retry => None,
            ColdResult::Verified(result) => Some(result),
            ColdResult::Reexecute {
                claim_guard,
                old_memo,
            } => {
                let old_memo = old_memo.downcast::<C>();

                if old_memo.value.is_none() {
                    return Some(VerifyResult::changed());
                }

                let memo = self.execute(db, claim_guard, Some(old_memo))?;
                let changed_at = memo.header.revisions.changed_at;

                // Always assume that a provisional value has changed.
                //
                // We don't know if a provisional value has actually changed. To determine whether
                // a provisional value has changed, we need to iterate the outer cycle, which cannot
                // be done here.
                Some(
                    if changed_at > revision || memo.header.may_be_provisional() {
                        VerifyResult::changed()
                    } else {
                        VerifyResult::unchanged_for_memo(&memo.header.revisions)
                    },
                )
            }
        }
    }
}

impl MemoHeader {
    fn maybe_changed_after_hot(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        revision: Revision,
        #[cfg(feature = "detailed-trace")] has_value: bool,
    ) -> Option<VerifyResult> {
        let can_shallow_update = self.shallow_verify_memo(
            zalsa,
            database_key_index,
            #[cfg(feature = "detailed-trace")]
            has_value,
        );
        if can_shallow_update.yes() && !self.may_be_provisional() {
            self.update_shallow(zalsa, database_key_index, can_shallow_update);

            Some(if self.revisions.changed_at > revision {
                VerifyResult::changed()
            } else {
                VerifyResult::unchanged_for_memo(&self.revisions)
            })
        } else {
            None
        }
    }

    /// Returns whether this memo is still valid in the current revision.
    pub(super) fn verify_memo(
        &self,
        db: crate::database::RawDatabase<'_>,
        claim_guard: &ClaimGuard<'_>,
        cycle_recovery_strategy: CycleRecoveryStrategy,
        #[cfg(feature = "detailed-trace")] has_value: bool,
    ) -> bool {
        let zalsa = claim_guard.zalsa();
        let zalsa_local = claim_guard.zalsa_local();
        let database_key_index = claim_guard.database_key_index();

        let can_shallow_update = self.shallow_verify_memo(
            zalsa,
            database_key_index,
            #[cfg(feature = "detailed-trace")]
            has_value,
        );
        if can_shallow_update.yes()
            && self.validate_may_be_provisional(
                zalsa,
                zalsa_local,
                database_key_index,
                #[cfg(feature = "detailed-trace")]
                has_value,
            )
        {
            self.update_shallow(zalsa, database_key_index, can_shallow_update);
            true
        } else {
            self.deep_verify_memo(
                db,
                claim_guard,
                cycle_recovery_strategy,
                #[cfg(feature = "detailed-trace")]
                has_value,
            )
            .is_unchanged()
        }
    }

    /// `Some` if the memo's value and `changed_at` time is still valid in this revision.
    /// Does only a shallow O(1) check, doesn't walk the dependencies.
    ///
    /// In general, a provisional memo (from cycle iteration) does not verify. Since we don't
    /// eagerly finalize all provisional memos in cycle iteration, we have to lazily check here
    /// (via `validate_provisional`) whether a may-be-provisional memo should actually be verified
    /// final, because its cycle heads are all now final.
    #[inline]
    pub(super) fn shallow_verify_memo(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        #[cfg(feature = "detailed-trace")] has_value: bool,
    ) -> ShallowUpdate {
        #[cfg(feature = "detailed-trace")]
        crate::tracing::debug!(
            "{database_key_index:?}: shallow_verify_memo(memo = {memo:#?})",
            memo = self.tracing_debug(has_value)
        );
        let verified_at = self.verified_at.load();
        let revision_now = zalsa.current_revision();

        if verified_at == revision_now {
            // Already verified.
            return ShallowUpdate::Verified;
        }

        self.shallow_verify_memo_cold(zalsa, database_key_index, verified_at)
    }

    #[cold]
    #[inline(never)]
    fn shallow_verify_memo_cold(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        verified_at: Revision,
    ) -> ShallowUpdate {
        let last_changed = zalsa.last_changed_revision(self.revisions.durability);
        crate::tracing::trace!(
            "{database_key_index:?}: check_durability({database_key_index:#?}, last_changed={:?} <= verified_at={:?}) = {:?}",
            last_changed,
            verified_at,
            last_changed <= verified_at,
        );
        if last_changed <= verified_at {
            // No input of the suitable durability has changed since last verified.
            ShallowUpdate::HigherDurability
        } else {
            ShallowUpdate::No
        }
    }

    #[inline]
    pub(super) fn update_shallow(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        update: ShallowUpdate,
    ) {
        if let ShallowUpdate::HigherDurability = update {
            self.mark_as_verified(zalsa, database_key_index);
            self.mark_outputs_as_verified(zalsa, database_key_index);
        }
    }

    /// Validates this memo if it is a provisional memo. Returns true for:
    /// * non provisional memos
    /// * provisional memos that have been successfully marked as verified final, that is, its
    ///   cycle heads have all been finalized.
    /// * provisional memos that have been created in the same revision and iteration and are part of the same cycle.
    #[inline]
    fn validate_may_be_provisional(
        &self,
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        #[cfg(feature = "detailed-trace")] has_value: bool,
    ) -> bool {
        if !self.may_be_provisional() {
            return true;
        }

        let cycle_heads = self.cycle_heads();

        if cycle_heads.is_empty() {
            return true;
        }

        #[cfg(feature = "detailed-trace")]
        crate::tracing::trace!(
            "{database_key_index:?}: validate_may_be_provisional(memo = {memo:#?})",
            memo = self.tracing_debug(has_value),
        );

        let verified_at = self.verified_at.load();
        validate_provisional(
            zalsa,
            database_key_index,
            &self.revisions,
            verified_at,
            cycle_heads,
        ) || validate_same_iteration(
            zalsa,
            zalsa_local,
            database_key_index,
            verified_at,
            cycle_heads,
        )
    }

    /// VerifyResult::Unchanged if the memo's value and `changed_at` time is up-to-date in the
    /// current revision. When this returns Unchanged with no cycle heads, it also updates the
    /// memo's `verified_at` field if needed to make future calls cheaper.
    ///
    /// Takes a [`ClaimGuard`] argument because this function recursively
    /// walks dependencies of `old_memo` and may even execute them to see if their
    /// outputs have changed.
    fn deep_verify_memo(
        &self,
        db: crate::database::RawDatabase<'_>,
        claim_guard: &ClaimGuard<'_>,
        cycle_recovery_strategy: CycleRecoveryStrategy,
        #[cfg(feature = "detailed-trace")] has_value: bool,
    ) -> VerifyResult {
        let zalsa = claim_guard.zalsa();
        let database_key_index = claim_guard.database_key_index();

        match self.origin() {
            QueryOriginRef::Derived(edges) => {
                #[cfg(feature = "detailed-trace")]
                crate::tracing::debug!(
                    "{database_key_index:?}: deep_verify_memo(old_memo = {old_memo:#?})",
                    old_memo = self.tracing_debug(has_value)
                );

                let is_provisional = self.may_be_provisional();

                // If the value is from the same revision but is still provisional, consider it changed
                // because we're now in a new iteration.
                if is_provisional {
                    return VerifyResult::changed();
                }

                // If the old memo participate in a cycle, but the query doesn't have cycle handling,
                // always return changed. The reasoning here is:
                //
                // * cycle heads flatten their dependecies. Therefore, no query with cycle handling
                //   participating in the same cycle should ever call `maybe_changed_after` on any other query.
                //   (we don't get here).
                // * the query can't be reached from any other query without cycle handling because,
                //   executing it would immediately panic because of the cycle.
                // * The only other place where we can reach this code is from `fetch`, this is when
                //   the outer cycle is being re-executed. Given that the cycle re-executes, this
                //   query must always be considered changed.
                //
                // For queries with cycle handling, verify the flattened
                // dependencies of the cycle head instead.
                if cycle_recovery_strategy == CycleRecoveryStrategy::Panic
                    && self.was_cycle_participant()
                {
                    return VerifyResult::changed();
                }

                let verified_at = self.verified_at.load();

                let result = deep_verify_edges(
                    db,
                    zalsa,
                    &self.revisions,
                    verified_at,
                    edges,
                    database_key_index,
                );

                if result.is_unchanged() {
                    self.mark_as_verified(zalsa, database_key_index);
                }

                result
            }

            QueryOriginRef::Assigned(_) => {
                // If the value was assigned by another query,
                // and that query were up-to-date,
                // then we would have updated the `verified_at` field already.
                // So the fact that we are here means that it was not specified
                // during this revision or is otherwise stale.
                //
                // Example of how this can happen:
                //
                // Conditionally specified queries
                // where the value is specified
                // in rev 1 but not in rev 2.
                VerifyResult::changed()
            }
            QueryOriginRef::DerivedUntracked(_) => {
                // Untracked inputs? Have to assume that it changed.
                VerifyResult::changed()
            }
        }
    }
}

fn maybe_changed_after_cold_cycle(
    zalsa_local: &ZalsaLocal,
    database_key_index: DatabaseKeyIndex,
    cycle_recovery_strategy: CycleRecoveryStrategy,
) -> VerifyResult {
    match cycle_recovery_strategy {
        // SAFETY: We do not access the query stack reentrantly.
        CycleRecoveryStrategy::Panic => unsafe {
            zalsa_local.with_query_stack_unchecked(|stack| {
                panic!(
                    "dependency graph cycle when validating {database_key_index:#?}, \
                    set cycle_fn/cycle_initial to fixpoint iterate.\n\
                    Query stack:\n{stack:#?}",
                );
            })
        },
        // We flatten the dependencies of queries with cycle handling that participate in a query.
        // Verifying those queries should never result in a cycle because all function dependencies were removed.
        // That means, if we hit this path, then some query introduced a new cycle that didn't exist
        // in the previous revision. We have to consider this query changed so that we ultimately
        // insert the fixpoint initial value in `fetch_cold_cycle`.
        CycleRecoveryStrategy::FallbackImmediate | CycleRecoveryStrategy::Fixpoint => {
            crate::tracing::debug!(
                "hit cycle at {database_key_index:?} in `maybe_changed_after`,  returning changed",
            );

            VerifyResult::changed()
        }
    }
}

fn deep_verify_edges(
    db: crate::database::RawDatabase,
    zalsa: &Zalsa,
    #[allow(unused)] old_revisions: &QueryRevisions,
    old_verified_at: Revision,
    edges: QueryEdges<'_>,
    database_key_index: DatabaseKeyIndex,
) -> VerifyResult {
    #[cfg(feature = "accumulator")]
    let mut inputs = InputAccumulatedValues::Empty;

    // Fully tracked inputs? Iterate over the inputs and check them, one by one.
    //
    // NB: It's important here that we are iterating the inputs in the order that
    // they executed. It's possible that if the value of some input I0 is no longer
    // valid, then some later input I1 might never have executed at all, so verifying
    // it is still up to date is meaningless.
    for edge in edges {
        match edge.kind() {
            QueryEdgeKind::Input => {
                let dependency_index = edge.key();
                let input_result = dependency_index.maybe_changed_after(db, zalsa, old_verified_at);

                match input_result {
                    VerifyResult::Changed => {
                        return VerifyResult::changed();
                    }
                    #[cfg(feature = "accumulator")]
                    VerifyResult::Unchanged { accumulated } => {
                        inputs |= accumulated;
                    }
                    #[cfg(not(feature = "accumulator"))]
                    VerifyResult::Unchanged { .. } => {}
                }
            }
            QueryEdgeKind::Output => {
                let dependency_index = edge.key();
                // Subtle: Mark outputs as validated now, even though we may
                // later find an input that requires us to re-execute the function.
                // Even if it re-execute, the function will wind up writing the same value,
                // since all prior inputs were green. It's important to do this during
                // this loop, because it's possible that one of our input queries will
                // re-execute and may read one of our earlier outputs
                // (e.g., in a scenario where we do something like
                // `e = Entity::new(..); query(e);` and `query` reads a field of `e`).
                //
                // NB. Accumulators are also outputs, but the above logic doesn't
                // quite apply to them. Since multiple values are pushed, the first value
                // may be unchanged, but later values could be different.
                // In that case, however, the data accumulated
                // by this function cannot be read until this function is marked green,
                // so even if we mark them as valid here, the function will re-execute
                // and overwrite the contents.
                dependency_index.mark_validated_output(zalsa, database_key_index);
            }
        }
    }
    let result = VerifyResult::unchanged_with_accumulated(
        #[cfg(feature = "accumulator")]
        inputs,
    );

    // This value is only read once the memo is verified. It's therefore safe
    // to write a non-final value here.
    #[cfg(feature = "accumulator")]
    old_revisions.accumulated_inputs.store(inputs);

    result
}

/// Check if this memo's cycle heads have all been finalized. If so, mark it verified final and
/// return true, if not return false.
fn validate_provisional(
    zalsa: &Zalsa,
    database_key_index: DatabaseKeyIndex,
    memo_revisions: &QueryRevisions,
    memo_verified_at: Revision,
    cycle_heads: &CycleHeads,
) -> bool {
    crate::tracing::trace!("{database_key_index:?}: validate_provisional({database_key_index:?})",);

    // Test if our cycle heads (with the same revision) are now finalized.
    for cycle_head in cycle_heads {
        let Some(provisional_status) = zalsa
            .lookup_ingredient(cycle_head.database_key_index.ingredient_index())
            .as_function()
            .and_then(|function| function.memo(zalsa, cycle_head.database_key_index.key_index()))
            .map(|memo| memo.header().provisional_status())
        else {
            return false;
        };

        match provisional_status {
            ProvisionalStatus::Provisional { .. } => return false,
            ProvisionalStatus::Final {
                iteration,
                verified_at,
                ..
            } => {
                // Only consider the cycle head if it is from the same revision as the memo
                if verified_at != memo_verified_at {
                    return false;
                }

                // It's important to also account for the iteration for the case where:
                // thread 1: `b` -> `a` (but only in the first iteration)
                //               -> `c` -> `b`
                // thread 2: `a` -> `b`
                //
                // If we don't account for the iteration, then `a` (from iteration 0) will be finalized
                // because its cycle head `b` is now finalized, but `b` never pulled `a` in the last iteration.
                if iteration != cycle_head.iteration.load() {
                    return false;
                }
            }
        }
    }
    // Relaxed is sufficient here because there are no other writes we need to ensure have
    // happened before marking this memo as verified-final.
    memo_revisions.verified_final.store(true, Ordering::Relaxed);
    true
}

/// If this is a provisional memo, validate that it was cached in the same iteration of the
/// same cycle(s) that we are still executing. If so, it is valid for reuse. This avoids
/// runaway re-execution of the same queries within a fixpoint iteration.
fn validate_same_iteration(
    zalsa: &Zalsa,
    zalsa_local: &ZalsaLocal,
    memo_database_key_index: DatabaseKeyIndex,
    memo_verified_at: Revision,
    cycle_heads: &CycleHeads,
) -> bool {
    crate::tracing::trace!("validate_same_iteration({memo_database_key_index:?})",);

    // This is an optimization to avoid unnecessary re-execution within the same revision.
    // Don't apply it when verifying memos from past revisions. We want them to re-execute
    // to verify their cycle heads and all participating queries.
    if memo_verified_at != zalsa.current_revision() {
        return false;
    }

    // Always return `false` for cycle initial values "unless" they are running in the same thread.
    if cycle_heads
        .iter_not_eq(memo_database_key_index)
        .next()
        .is_none()
    {
        // SAFETY: We do not access the query stack reentrantly.
        let on_stack = unsafe {
            zalsa_local.with_query_stack_unchecked(|stack| {
                stack
                    .iter()
                    .rev()
                    .any(|query| query.database_key_index == memo_database_key_index)
            })
        };

        return on_stack;
    }

    let cycle_heads_iter = TryClaimCycleHeadsIter::new(zalsa, cycle_heads);

    for cycle_head in cycle_heads_iter {
        match cycle_head {
            TryClaimHeadsResult::Cycle {
                head_iteration,
                memo_iteration,
                verified_at: head_verified_at,
            } => {
                if head_verified_at != memo_verified_at {
                    return false;
                }

                if head_iteration != memo_iteration {
                    return false;
                }
            }
            _ => {
                return false;
            }
        }
    }

    true
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub(super) enum ShallowUpdate {
    /// The memo is from this revision and has already been verified
    Verified,

    /// The revision for the memo's durability hasn't changed. It can be marked as verified
    /// in this revision.
    HigherDurability,

    /// The memo requires a deep verification.
    No,
}

impl ShallowUpdate {
    pub(super) fn yes(&self) -> bool {
        matches!(
            self,
            ShallowUpdate::Verified | ShallowUpdate::HigherDurability
        )
    }
}
