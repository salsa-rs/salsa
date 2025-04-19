use std::sync::atomic::Ordering;

use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::cycle::{CycleHeads, CycleRecoveryStrategy};
use crate::function::fetch::LazyActiveQueryGuard;
use crate::function::memo::Memo;
use crate::function::{Configuration, IngredientImpl};
use crate::key::DatabaseKeyIndex;
use crate::table::sync::ClaimResult;
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::{QueryEdge, QueryOrigin};
use crate::{AsDynDatabase as _, Id, Revision};

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
            let Some(memo) = memo_guard else {
                // No memo? Assume has changed.
                return VerifyResult::Changed;
            };

            if let Some(shallow_update) = self.shallow_verify_memo(zalsa, database_key_index, memo)
            {
                if self.validate_may_be_provisional(db, zalsa, database_key_index, memo) {
                    self.update_shallow(db, zalsa, database_key_index, memo, shallow_update);

                    return if memo.revisions.changed_at > revision {
                        VerifyResult::Changed
                    } else {
                        VerifyResult::Unchanged(
                            memo.revisions.accumulated_inputs.load(),
                            CycleHeads::default(),
                        )
                    };
                }
            }

            if let Some(mcs) =
                self.maybe_changed_after_cold(zalsa, db, id, revision, memo_ingredient_index)
            {
                return mcs;
            } else {
                // We failed to claim, have to retry.
            }
        }
    }

    #[inline(never)]
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
                CycleRecoveryStrategy::Panic => db
                    .zalsa_local()
                    .cycle_panic(database_key_index, "validating"),
                CycleRecoveryStrategy::FallbackImmediate => {
                    db.zalsa_local()
                        .assert_top_non_panic_cycle(database_key_index);
                    return Some(VerifyResult::unchanged());
                }
                CycleRecoveryStrategy::Fixpoint => {
                    return Some(VerifyResult::Unchanged(
                        InputAccumulatedValues::Empty,
                        CycleHeads::initial(database_key_index),
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
        let mut active_query = LazyActiveQueryGuard::new(database_key_index);
        if let VerifyResult::Unchanged(_, cycle_heads) =
            self.deep_verify_memo(db, zalsa, old_memo, &mut active_query)
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
            let memo = self.execute(
                db,
                active_query.into_inner(db.zalsa_local(), C::CYCLE_STRATEGY),
                Some(old_memo),
            );
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
        memo: &Memo<C::Output<'_>>,
    ) -> Option<ShallowUpdate> {
        tracing::debug!(
            "{database_key_index:?}: shallow_verify_memo(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );
        let verified_at = memo.verified_at.load();
        let revision_now = zalsa.current_revision();

        if verified_at == revision_now {
            // Already verified.
            return Some(ShallowUpdate::Verified);
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
            Some(ShallowUpdate::HigherDurability(revision_now))
        } else {
            None
        }
    }

    #[inline]
    pub(super) fn update_shallow(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<C::Output<'_>>,
        update: ShallowUpdate,
    ) {
        if let ShallowUpdate::HigherDurability(revision_now) = update {
            memo.mark_as_verified(db, revision_now, database_key_index);
            memo.mark_outputs_as_verified(zalsa, db.as_dyn_database(), database_key_index);
        }
    }

    /// Validates this memo if it is a provisional memo. Returns true for non provisional memos or
    /// if the provisional memo has been successfully marked as verified final, that is, its
    /// cycle heads have all been finalized.
    #[inline]
    pub(super) fn validate_may_be_provisional(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<C::Output<'_>>,
    ) -> bool {
        // Wouldn't it be nice if rust had an implication operator ...
        // may_be_provisional -> validate_provisional
        !memo.may_be_provisional() || self.validate_provisional(db, zalsa, database_key_index, memo)
    }

    /// Check if this memo's cycle heads have all been finalized. If so, mark it verified final and
    /// return true, if not return false.
    #[inline]
    fn validate_provisional(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<C::Output<'_>>,
    ) -> bool {
        tracing::trace!(
            "{database_key_index:?}: validate_provisional(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );
        if (&memo.revisions.cycle_heads).into_iter().any(|cycle_head| {
            zalsa
                .lookup_ingredient(cycle_head.database_key_index.ingredient_index())
                .is_provisional_cycle_head(
                    db.as_dyn_database(),
                    cycle_head.database_key_index.key_index(),
                )
        }) {
            return false;
        }
        // Relaxed is sufficient here because there are no other writes we need to ensure have
        // happened before marking this memo as verified-final.
        memo.revisions.verified_final.store(true, Ordering::Relaxed);
        true
    }

    /// If this is a provisional memo, validate that it was cached in the same iteration of the
    /// same cycle(s) that we are still executing. If so, it is valid for reuse. This avoids
    /// runaway re-execution of the same queries within a fixpoint iteration.
    pub(super) fn validate_same_iteration(
        &self,
        db: &C::DbView,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<C::Output<'_>>,
    ) -> bool {
        tracing::trace!(
            "{database_key_index:?}: validate_same_iteration(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );

        if memo.revisions.cycle_heads.is_empty() {
            return true;
        }

        let cycle_heads = &memo.revisions.cycle_heads;

        db.zalsa_local().with_query_stack(|stack| {
            cycle_heads.iter().all(|cycle_head| {
                stack.iter().rev().any(|query| {
                    query.database_key_index == cycle_head.database_key_index
                        && query.iteration_count() == cycle_head.iteration_count
                })
            })
        })
    }

    /// VerifyResult::Unchanged if the memo's value and `changed_at` time is up-to-date in the
    /// current revision. When this returns Unchanged with no cycle heads, it also updates the
    /// memo's `verified_at` field if needed to make future calls cheaper.
    ///
    /// Takes an [`ActiveQueryGuard`] argument because this function recursively
    /// walks dependencies of `old_memo` and may even execute them to see if their
    /// outputs have changed.
    pub(super) fn deep_verify_memo<'db>(
        &self,
        db: &'db C::DbView,
        zalsa: &Zalsa,
        old_memo: &Memo<C::Output<'_>>,
        active_query: &mut LazyActiveQueryGuard<'db>,
    ) -> VerifyResult {
        let database_key_index = active_query.database_key_index();

        tracing::debug!(
            "{database_key_index:?}: deep_verify_memo(old_memo = {old_memo:#?})",
            old_memo = old_memo.tracing_debug()
        );

        let shallow_update = self.shallow_verify_memo(zalsa, database_key_index, old_memo);
        let shallow_update_possible = shallow_update.is_some();
        if let Some(shallow_update) = shallow_update {
            if self.validate_may_be_provisional(db, zalsa, database_key_index, old_memo)
                || self.validate_same_iteration(db, database_key_index, old_memo)
            {
                self.update_shallow(db, zalsa, database_key_index, old_memo, shallow_update);

                return VerifyResult::unchanged();
            }
        }

        match &old_memo.revisions.origin {
            QueryOrigin::Assigned(_) => {
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
                VerifyResult::Changed
            }
            QueryOrigin::FixpointInitial if old_memo.may_be_provisional() => VerifyResult::Changed,
            QueryOrigin::FixpointInitial => VerifyResult::unchanged(),
            QueryOrigin::DerivedUntracked(_) => {
                // Untracked inputs? Have to assume that it changed.
                VerifyResult::Changed
            }
            QueryOrigin::Derived(edges) => {
                let is_provisional = old_memo.may_be_provisional();

                // If the value is from the same revision but is still provisional, consider it changed
                if shallow_update_possible && is_provisional {
                    return VerifyResult::Changed;
                }

                let _guard = active_query.guard(db.zalsa_local(), C::CYCLE_STRATEGY);

                let mut cycle_heads = CycleHeads::default();
                'cycle: loop {
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
                                match dependency_index.maybe_changed_after(dyn_db, last_verified_at)
                                {
                                    VerifyResult::Changed => break 'cycle VerifyResult::Changed,
                                    VerifyResult::Unchanged(input_accumulated, cycles) => {
                                        cycle_heads.extend(&cycles);
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

                    let in_heads = cycle_heads.remove(&database_key_index);

                    if cycle_heads.is_empty() {
                        old_memo.mark_as_verified(db, zalsa.current_revision(), database_key_index);
                        old_memo.revisions.accumulated_inputs.store(inputs);

                        if is_provisional {
                            old_memo
                                .revisions
                                .verified_final
                                .store(true, Ordering::Relaxed);
                        }

                        if in_heads {
                            continue 'cycle;
                        }
                    }
                    break 'cycle VerifyResult::Unchanged(inputs, cycle_heads);
                }
            }
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub(super) enum ShallowUpdate {
    /// The memo is from this revision and has already been verified
    Verified,

    /// The revision for the memo's durability hasn't changed. It can be marked as verified
    /// in this revision.
    HigherDurability(Revision),
}
