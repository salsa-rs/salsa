use crate::{
    cycle::CycleRecoveryStrategy,
    key::DatabaseKeyIndex,
    table::sync::ClaimResult,
    zalsa::{Zalsa, ZalsaDatabase},
    zalsa_local::{ActiveQueryGuard, QueryEdge, QueryOrigin},
    AsDynDatabase as _, Id, Revision,
};
use rustc_hash::FxHashSet;

use super::{memo::Memo, Configuration, IngredientImpl};

/// Result of memo validation.
pub enum VerifyResult {
    /// Memo has changed and needs to be recomputed.
    Changed,

    /// Memo remains valid.
    ///
    /// Database keys in the hashset represent cycle heads encountered in validation; don't mark
    /// memos verified until we've iterated the full cycle to ensure no inputs changed.
    Unchanged(FxHashSet<DatabaseKeyIndex>),
}

impl VerifyResult {
    pub(crate) fn changed_if(condition: bool) -> Self {
        if condition {
            Self::Changed
        } else {
            Self::unchanged()
        }
    }

    pub(crate) fn unchanged() -> Self {
        Self::Unchanged(FxHashSet::default())
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
        zalsa_local.unwind_if_revision_cancelled(db.as_dyn_database());

        loop {
            let database_key_index = self.database_key_index(id);

            tracing::debug!("{database_key_index:?}: maybe_changed_after(revision = {revision:?})");

            // Check if we have a verified version: this is the hot path.
            let memo_guard = self.get_memo_from_table_for(zalsa, id);
            if let Some(memo) = &memo_guard {
                if self.shallow_verify_memo(db, zalsa, database_key_index, memo, false) {
                    return VerifyResult::changed_if(memo.revisions.changed_at > revision);
                }
                drop(memo_guard); // release the arc-swap guard before cold path
                if let Some(mcs) = self.maybe_changed_after_cold(db, id, revision) {
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
        db: &'db C::DbView,
        key_index: Id,
        revision: Revision,
    ) -> Option<VerifyResult> {
        let (zalsa, zalsa_local) = db.zalsas();
        let database_key_index = self.database_key_index(key_index);

        let _claim_guard = match zalsa.sync_table_for(key_index).claim(
            db.as_dyn_database(),
            zalsa_local,
            database_key_index,
            self.memo_ingredient_index,
        ) {
            ClaimResult::Retry => return None,
            ClaimResult::Cycle => match C::CYCLE_STRATEGY {
                CycleRecoveryStrategy::Panic => panic!(
                    "dependency graph cycle validating {database_key_index:#?}; \
                     set cycle_fn/cycle_initial to fixpoint iterate"
                ),
                CycleRecoveryStrategy::Fixpoint => {
                    return Some(VerifyResult::Unchanged(FxHashSet::from_iter([
                        database_key_index,
                    ])))
                }
            },
            ClaimResult::Claimed(guard) => guard,
        };
        // Load the current memo, if any.
        let Some(old_memo) = self.get_memo_from_table_for(zalsa, key_index) else {
            return Some(VerifyResult::Changed);
        };

        tracing::debug!(
            "{database_key_index:?}: maybe_changed_after_cold, successful claim, \
                revision = {revision:?}, old_memo = {old_memo:#?}",
            old_memo = old_memo.tracing_debug()
        );

        // Check if the inputs are still valid and we can just compare `changed_at`.
        let active_query = zalsa_local.push_query(database_key_index);
        if let VerifyResult::Unchanged(cycle_heads) =
            self.deep_verify_memo(db, &old_memo, &active_query)
        {
            return Some(if old_memo.revisions.changed_at > revision {
                VerifyResult::Changed
            } else {
                VerifyResult::Unchanged(cycle_heads)
            });
        }

        // If inputs have changed, but we have an old value, we can re-execute.
        // It is possible the result will be equal to the old value and hence
        // backdated. In that case, although we will have computed a new memo,
        // the value has not logically changed.
        if old_memo.value.is_some() {
            let memo = self.execute(db, database_key_index, Some(old_memo));
            let changed_at = memo.revisions.changed_at;
            return Some(VerifyResult::changed_if(changed_at > revision));
        }

        // Otherwise, nothing for it: have to consider the value to have changed.
        Some(VerifyResult::Changed)
    }

    /// True if the memo's value and `changed_at` time is still valid in this revision.
    /// Does only a shallow O(1) check, doesn't walk the dependencies.
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
        if !allow_provisional {
            if memo.may_be_provisional() {
                tracing::debug!(
                    "{database_key_index:?}: validate_provisional(memo = {memo:#?})",
                    memo = memo.tracing_debug()
                );
                if !self.validate_provisional(db, zalsa, memo) {
                    return false;
                }
            }
        }
        let verified_at = memo.verified_at.load();
        let revision_now = zalsa.current_revision();

        if verified_at == revision_now {
            // Already verified.
            return true;
        }

        if memo.check_durability(zalsa) {
            // No input of the suitable durability has changed since last verified.
            let db = db.as_dyn_database();
            memo.mark_as_verified(db, revision_now, database_key_index);
            memo.mark_outputs_as_verified(db, database_key_index);
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
        for cycle_head in &memo.revisions.cycle_heads {
            if !zalsa
                .lookup_ingredient(cycle_head.ingredient_index)
                .is_verified_final(db.as_dyn_database(), cycle_head.key_index)
            {
                return false;
            }
        }
        memo.verified_final.store(true);
        true
    }

    /// VerifyResult::Unchanged if the memo's value and `changed_at` time is up to date in the
    /// current revision. When this returns Unchanged with no cycle heads, it also updates the
    /// memo's `verified_at` field if needed to make future calls cheaper.
    ///
    /// Takes an [`ActiveQueryGuard`] argument because this function recursively
    /// walks dependencies of `old_memo` and may even execute them to see if their
    /// outputs have changed.
    pub(super) fn deep_verify_memo(
        &self,
        db: &C::DbView,
        old_memo: &Memo<C::Output<'_>>,
        active_query: &ActiveQueryGuard<'_>,
    ) -> VerifyResult {
        let zalsa = db.zalsa();
        let database_key_index = active_query.database_key_index;

        tracing::debug!(
            "{database_key_index:?}: deep_verify_memo(old_memo = {old_memo:#?})",
            old_memo = old_memo.tracing_debug()
        );

        if self.shallow_verify_memo(db, zalsa, database_key_index, old_memo, false) {
            return VerifyResult::Unchanged(Default::default());
        }
        if old_memo.may_be_provisional() {
            return VerifyResult::Changed;
        }

        loop {
            let mut cycle_heads = FxHashSet::default();

            match &old_memo.revisions.origin {
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
                QueryOrigin::BaseInput | QueryOrigin::FixpointInitial => {
                    // This value was `set` by the mutator thread -- ie, it's a base input and it cannot be out of date.
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
                    for &edge in edges.input_outputs.iter() {
                        match edge {
                            QueryEdge::Input(dependency_index) => {
                                match dependency_index
                                    .maybe_changed_after(db.as_dyn_database(), last_verified_at)
                                {
                                    VerifyResult::Changed => return VerifyResult::Changed,
                                    VerifyResult::Unchanged(cycles) => cycle_heads.extend(cycles),
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
                                //
                                // TODO not if we found a cycle head other than ourself?
                                dependency_index.mark_validated_output(
                                    db.as_dyn_database(),
                                    database_key_index,
                                );
                            }
                        }
                    }
                }
            }

            let in_heads = cycle_heads.remove(&database_key_index);

            if cycle_heads.is_empty() {
                old_memo.mark_as_verified(
                    db.as_dyn_database(),
                    zalsa.current_revision(),
                    database_key_index,
                );
            }
            if in_heads {
                continue;
            }
            return VerifyResult::Unchanged(cycle_heads);
        }
    }
}
