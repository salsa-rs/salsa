use crate::{
    accumulator::accumulated_map::InputAccumulatedValues,
    cycle::{CycleHeads, CycleRecoveryStrategy},
    key::DatabaseKeyIndex,
    table::sync::ClaimResult,
    zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase},
    zalsa_local::{ActiveQueryGuard, QueryEdge, QueryOrigin},
    AsDynDatabase as _, Id, Revision,
};
use std::sync::atomic::Ordering;

use super::{memo::Memo, Configuration, IngredientImpl};

/// Result of memo validation.
pub enum VerifyResult {
    /// Memo has changed and needs to be recomputed.
    Changed,

    /// Memo remains valid.
    ///
    /// The first inner value tracks whether the memo or any of its dependencies have an
    /// accumulated value.
    ///
    /// The second is the cycle heads encountered in validation; don't mark
    /// memos verified until we've iterated the full cycle to ensure no inputs changed.
    Unchanged(InputAccumulatedValues, CycleHeads),
}

impl VerifyResult {
    pub(crate) fn changed_if(changed: bool) -> Self {
        if changed {
            Self::Changed
        } else {
            Self::unchanged()
        }
    }

    pub(crate) fn unchanged() -> Self {
        Self::Unchanged(InputAccumulatedValues::Empty, CycleHeads::default())
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
        let zalsa = db.zalsa();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);
        zalsa.unwind_if_revision_cancelled(db);

        loop {
            let database_key_index = self.database_key_index(id);

            tracing::debug!("{database_key_index:?}: maybe_changed_after(revision = {revision:?})");

            // Check if we have a verified version: this is the hot path.
            let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
            if let Some(memo) = memo_guard {
                if self.shallow_verify_memo(db, zalsa, database_key_index, memo, false) {
                    return if memo.revisions.changed_at > revision {
                        VerifyResult::Changed
                    } else {
                        VerifyResult::Unchanged(
                            memo.revisions.accumulated_inputs.load(),
                            CycleHeads::default(),
                        )
                    };
                }
                if let Some(mcs) =
                    self.maybe_changed_after_cold(zalsa, db, id, revision, memo_ingredient_index)
                {
                    return mcs;
                } else {
                    // We failed to claim, have to retry.
                }
            } else {
                // No memo? Assume has changed.
                return VerifyResult::Changed;
            }
        }
    }

    fn maybe_changed_after_cold<'db>(
        &'db self,
        zalsa: &Zalsa,
        db: &'db C::DbView,
        key_index: Id,
        revision: Revision,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<VerifyResult> {
        let database_key_index = self.database_key_index(key_index);

        let _claim_guard = match zalsa.sync_table_for(key_index).claim(
            db,
            zalsa,
            database_key_index,
            memo_ingredient_index,
        ) {
            ClaimResult::Retry => return None,
            ClaimResult::Cycle => match C::CYCLE_STRATEGY {
                CycleRecoveryStrategy::Panic => panic!(
                    "dependency graph cycle validating {database_key_index:#?}; \
                     set cycle_fn/cycle_initial to fixpoint iterate"
                ),
                CycleRecoveryStrategy::Fixpoint => {
                    return Some(VerifyResult::Unchanged(
                        InputAccumulatedValues::Empty,
                        CycleHeads::from(database_key_index),
                    ));
                }
            },
            ClaimResult::Claimed(guard) => guard,
        };
        // Load the current memo, if any.
        let Some(old_memo) = self.get_memo_from_table_for(zalsa, key_index, memo_ingredient_index)
        else {
            return Some(VerifyResult::Changed);
        };

        tracing::debug!(
            "{database_key_index:?}: maybe_changed_after_cold, successful claim, \
                revision = {revision:?}, old_memo = {old_memo:#?}",
            old_memo = old_memo.tracing_debug()
        );

        // Check if the inputs are still valid. We can just compare `changed_at`.
        let active_query = db.zalsa_local().push_query(database_key_index);
        if let VerifyResult::Unchanged(_, cycle_heads) =
            self.deep_verify_memo(db, zalsa, old_memo, &active_query)
        {
            return Some(if old_memo.revisions.changed_at > revision {
                VerifyResult::Changed
            } else {
                VerifyResult::Unchanged(old_memo.revisions.accumulated_inputs.load(), cycle_heads)
            });
        }

        // If inputs have changed, but we have an old value, we can re-execute.
        // It is possible the result will be equal to the old value and hence
        // backdated. In that case, although we will have computed a new memo,
        // the value has not logically changed.
        if old_memo.value.is_some() {
            let memo = self.execute(db, active_query, Some(old_memo));
            let changed_at = memo.revisions.changed_at;

            return Some(if changed_at > revision {
                VerifyResult::Changed
            } else {
                VerifyResult::Unchanged(
                    match &memo.revisions.accumulated {
                        Some(_) => InputAccumulatedValues::Any,
                        None => memo.revisions.accumulated_inputs.load(),
                    },
                    CycleHeads::default(),
                )
            });
        }

        // Otherwise, nothing for it: have to consider the value to have changed.
        Some(VerifyResult::Changed)
    }

    /// True if the memo's value and `changed_at` time is still valid in this revision.
    /// Does only a shallow O(1) check, doesn't walk the dependencies.
    ///
    /// In general, a provisional memo (from cycle iteration) does not verify. Since we don't
    /// eagerly finalize all provisional memos in cycle iteration, we have to lazily check here
    /// (via `validate_provisional`) whether a may-be-provisional memo should actually be verified
    /// final, because its cycle heads are all now final.
    ///
    /// If `allow_provisional` is `true`, don't check provisionality and return whatever memo we
    /// find that can be verified in this revision, whether provisional or not. This only occurs at
    /// one call-site, in `fetch_cold` when we actually encounter a cycle, and want to check if
    /// there is an existing provisional memo we can reuse.
    #[inline]
    pub(super) fn shallow_verify_memo(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<C::Output<'_>>,
        allow_provisional: bool,
    ) -> bool {
        tracing::debug!(
            "{database_key_index:?}: shallow_verify_memo(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );
        if !allow_provisional && memo.may_be_provisional() {
            tracing::debug!(
                "{database_key_index:?}: validate_provisional(memo = {memo:#?})",
                memo = memo.tracing_debug()
            );
            if !self.validate_provisional(db, zalsa, memo) {
                return false;
            }
        }
        let verified_at = memo.verified_at.load();
        let revision_now = zalsa.current_revision();

        if verified_at == revision_now {
            // Already verified.
            return true;
        }

        let last_changed = zalsa.last_changed_revision(memo.revisions.durability);
        tracing::debug!(
            "{database_key_index:?}: check_durability(memo = {memo:#?}, last_changed={:?} <= verified_at={:?}) = {:?}",
            last_changed,
            verified_at,
            last_changed <= verified_at,
            memo = memo.tracing_debug()
        );
        if last_changed <= verified_at {
            // No input of the suitable durability has changed since last verified.
            memo.mark_as_verified(
                db,
                revision_now,
                database_key_index,
                memo.revisions.accumulated_inputs.load(),
            );
            memo.mark_outputs_as_verified(zalsa, db.as_dyn_database(), database_key_index);
            return true;
        }

        false
    }

    /// Check if this memo's cycle heads have all been finalized. If so, mark it verified final and
    /// return true, if not return false.
    fn validate_provisional(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        memo: &Memo<C::Output<'_>>,
    ) -> bool {
        if (&memo.revisions.cycle_heads).into_iter().any(|cycle_head| {
            zalsa
                .lookup_ingredient(cycle_head.ingredient_index)
                .is_provisional_cycle_head(db.as_dyn_database(), cycle_head.key_index)
        }) {
            return false;
        }
        // Relaxed is sufficient here because there are no other writes we need to ensure have
        // happened before marking this memo as verified-final.
        memo.verified_final.store(true, Ordering::Relaxed);
        true
    }

    /// VerifyResult::Unchanged if the memo's value and `changed_at` time is up-to-date in the
    /// current revision. When this returns Unchanged with no cycle heads, it also updates the
    /// memo's `verified_at` field if needed to make future calls cheaper.
    ///
    /// Takes an [`ActiveQueryGuard`] argument because this function recursively
    /// walks dependencies of `old_memo` and may even execute them to see if their
    /// outputs have changed.
    pub(super) fn deep_verify_memo(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        old_memo: &Memo<C::Output<'_>>,
        active_query: &ActiveQueryGuard<'_>,
    ) -> VerifyResult {
        let database_key_index = active_query.database_key_index;

        tracing::debug!(
            "{database_key_index:?}: deep_verify_memo(old_memo = {old_memo:#?})",
            old_memo = old_memo.tracing_debug()
        );

        if self.shallow_verify_memo(db, zalsa, database_key_index, old_memo, false) {
            return VerifyResult::Unchanged(InputAccumulatedValues::Empty, Default::default());
        }
        if old_memo.may_be_provisional() {
            return VerifyResult::Changed;
        }

        let mut cycle_heads = vec![];
        loop {
            let inputs = match &old_memo.revisions.origin {
                QueryOrigin::Assigned(_) => {
                    // If the value was assigneed by another query,
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
                    return VerifyResult::Changed;
                }
                QueryOrigin::FixpointInitial => {
                    return VerifyResult::unchanged();
                }
                QueryOrigin::DerivedUntracked(_) => {
                    // Untracked inputs? Have to assume that it changed.
                    return VerifyResult::Changed;
                }
                QueryOrigin::Derived(edges) => {
                    // Fully tracked inputs? Iterate over the inputs and check them, one by one.
                    //
                    // NB: It's important here that we are iterating the inputs in the order that
                    // they executed. It's possible that if the value of some input I0 is no longer
                    // valid, then some later input I1 might never have executed at all, so verifying
                    // it is still up to date is meaningless.
                    let last_verified_at = old_memo.verified_at.load();
                    let mut inputs = InputAccumulatedValues::Empty;
                    let dyn_db = db.as_dyn_database();
                    for &edge in edges.input_outputs.iter() {
                        match edge {
                            QueryEdge::Input(dependency_index) => {
                                match dependency_index
                                    .maybe_changed_after(db.as_dyn_database(), last_verified_at)
                                {
                                    VerifyResult::Changed => return VerifyResult::Changed,
                                    VerifyResult::Unchanged(input_accumulated, cycles) => {
                                        cycles.insert_into(&mut cycle_heads);
                                        inputs |= input_accumulated;
                                    }
                                }
                            }
                            QueryEdge::Output(dependency_index) => {
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
                                dependency_index.mark_validated_output(
                                    zalsa,
                                    dyn_db,
                                    database_key_index,
                                );
                            }
                        }
                    }
                    inputs
                }
            };

            // Possible scenarios here:
            //
            // 1. Cycle heads is empty. We traversed our full dependency graph and neither hit any
            //    cycles, nor found any changed dependencies. We can mark our memo verified and
            //    return Unchanged with empty cycle heads.
            //
            // 2. Cycle heads is non-empty, and does not contain our own key index. We are part of
            //    a cycle, and since we don't know if some other cycle participant that hasn't been
            //    traversed yet (that is, some other dependency of the cycle head, which is only a
            //    dependency of ours via the cycle) might still have changed, we can't yet mark our
            //    memo verified. We can return a provisional Unchanged, with cycle heads.
            //
            // 3. Cycle heads is non-empty, and contains only our own key index. We are the head of
            //    a cycle, and we've now traversed the entire cycle and found no changes, but no
            //    other cycle participants were verified (they would have all hit case 2 above). We
            //    can now safely mark our own memo as verified. Then we have to traverse the entire
            //    cycle again. This time, since our own memo is verified, there will be no cycle
            //    encountered, and the rest of the cycle will be able to verify itself.
            //
            // 4. Cycle heads is non-empty, and contains our own key index as well as other key
            //    indices. We are the head of a cycle nested within another cycle. We can't mark
            //    our own memo verified (for the same reason as in case 2: the full outer cycle
            //    hasn't been validated unchanged yet). We return Unchanged, with ourself removed
            //    from cycle heads. We will handle our own memo (and the rest of our cycle) on a
            //    future iteration; first the outer cycle head needs to verify itself.

            let in_heads = cycle_heads
                .iter()
                .position(|&head| head == database_key_index)
                .inspect(|&head| _ = cycle_heads.swap_remove(head))
                .is_some();

            if cycle_heads.is_empty() {
                old_memo.mark_as_verified(db, zalsa.current_revision(), database_key_index, inputs);

                if in_heads {
                    // Iterate our dependency graph again, starting from the top. We clear the
                    // cycle heads here because we are starting a fresh traversal. (It might be
                    // logically clearer to create a new HashSet each time, but clearing the
                    // existing one is more efficient.)
                    cycle_heads.clear();
                    continue;
                }
            }
            return VerifyResult::Unchanged(
                InputAccumulatedValues::Empty,
                CycleHeads::from(cycle_heads),
            );
        }
    }
}
