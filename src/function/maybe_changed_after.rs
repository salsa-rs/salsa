use crate::{
    accumulator::accumulated_map::InputAccumulatedValues,
    ingredient::MaybeChangedAfter,
    key::DatabaseKeyIndex,
    zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase},
    zalsa_local::{ActiveQueryGuard, QueryEdge, QueryOrigin},
    AsDynDatabase as _, Id, Revision,
};

use super::{memo::Memo, Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn maybe_changed_after<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
        revision: Revision,
    ) -> MaybeChangedAfter {
        let zalsa = db.zalsa();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);
        zalsa.unwind_if_revision_cancelled(db);

        loop {
            let database_key_index = self.database_key_index(id);

            tracing::debug!("{database_key_index:?}: maybe_changed_after(revision = {revision:?})");

            // Check if we have a verified version: this is the hot path.
            let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
            if let Some(memo) = memo_guard {
                if self.shallow_verify_memo(db, zalsa, database_key_index, memo) {
                    return if memo.revisions.changed_at > revision {
                        MaybeChangedAfter::Yes
                    } else {
                        MaybeChangedAfter::No(memo.revisions.accumulated_inputs.load())
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
                return MaybeChangedAfter::Yes;
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
    ) -> Option<MaybeChangedAfter> {
        let database_key_index = self.database_key_index(key_index);

        let _claim_guard = zalsa.sync_table_for(key_index).claim(
            db,
            zalsa,
            database_key_index,
            memo_ingredient_index,
        )?;
        let active_query = db.zalsa_local().push_query(database_key_index);

        // Load the current memo, if any.
        let Some(old_memo) = self.get_memo_from_table_for(zalsa, key_index, memo_ingredient_index)
        else {
            return Some(MaybeChangedAfter::Yes);
        };

        tracing::debug!(
            "{database_key_index:?}: maybe_changed_after_cold, successful claim, \
                revision = {revision:?}, old_memo = {old_memo:#?}",
            old_memo = old_memo.tracing_debug()
        );

        // Check if the inputs are still valid. We can just compare `changed_at`.
        if self.deep_verify_memo(db, zalsa, old_memo, &active_query) {
            return Some(if old_memo.revisions.changed_at > revision {
                MaybeChangedAfter::Yes
            } else {
                MaybeChangedAfter::No(old_memo.revisions.accumulated_inputs.load())
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
                MaybeChangedAfter::Yes
            } else {
                MaybeChangedAfter::No(match &memo.revisions.accumulated {
                    Some(_) => InputAccumulatedValues::Any,
                    None => memo.revisions.accumulated_inputs.load(),
                })
            });
        }

        // Otherwise, nothing for it: have to consider the value to have changed.
        Some(MaybeChangedAfter::Yes)
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
    ) -> bool {
        let verified_at = memo.verified_at.load();
        let revision_now = zalsa.current_revision();

        tracing::debug!(
            "{database_key_index:?}: shallow_verify_memo(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );

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

    /// True if the memo's value and `changed_at` time is up-to-date in the current
    /// revision. When this returns true, it also updates the memo's `verified_at`
    /// field if needed to make future calls cheaper.
    ///
    /// Takes an [`ActiveQueryGuard`] argument because this function recursively
    /// walks dependencies of `old_memo` and may even execute them to see if their
    /// outputs have changed. As that could lead to cycles, it is important that the
    /// query is on the stack.
    pub(super) fn deep_verify_memo(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        old_memo: &Memo<C::Output<'_>>,
        active_query: &ActiveQueryGuard<'_>,
    ) -> bool {
        let database_key_index = active_query.database_key_index;

        tracing::debug!(
            "{database_key_index:?}: deep_verify_memo(old_memo = {old_memo:#?})",
            old_memo = old_memo.tracing_debug()
        );

        if self.shallow_verify_memo(db, zalsa, database_key_index, old_memo) {
            return true;
        }

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
                false
            }
            QueryOrigin::DerivedUntracked(_) => {
                // Untracked inputs? Have to assume that it changed.
                false
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
                            match dependency_index.maybe_changed_after(dyn_db, last_verified_at) {
                                MaybeChangedAfter::Yes => return false,
                                MaybeChangedAfter::No(input_accumulated) => {
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

                old_memo.mark_as_verified(db, zalsa.current_revision(), database_key_index, inputs);
                true
            }
        }
    }
}
