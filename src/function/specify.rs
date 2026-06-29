#[cfg(feature = "accumulator")]
use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::active_query::CompletedQuery;
use crate::function::memo::{Memo, MemoHeader};
use crate::function::sync::{ClaimResult, Reentrancy};
use crate::function::{Configuration, IngredientImpl};
use crate::revision::AtomicRevision;
use crate::sync::atomic::AtomicBool;
use crate::tracked_struct::TrackedStructInDb;
use crate::zalsa::{Zalsa, ZalsaDatabase};
use crate::zalsa_local::{OriginAndExtra, QueryOriginRef, QueryRevisions};
use crate::{DatabaseKeyIndex, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Specify the value for `key` *and* record that we did so.
    /// Used for explicit calls to `specify`, but not needed for pre-declared tracked struct fields.
    pub fn specify_and_record<'db>(&'db self, db: &'db C::DbView, key: Id, value: C::Output<'db>)
    where
        C::Input<'db>: TrackedStructInDb,
    {
        let (zalsa, zalsa_local) = db.zalsas();

        let (active_query_key, current_deps, cycle_heads) =
            match zalsa_local.active_query_with_cycle_heads() {
                Some(v) => v,
                None => panic!("can only use `specify` inside a tracked function"),
            };

        // `specify` only works if the key is a tracked struct created in the current query.
        //
        // The reason is this. We want to ensure that the same result is reached regardless of
        // the "path" that the user takes through the execution graph.
        // If you permit values to be specified from other queries, you can have a situation like this:
        // * Q0 creates the tracked struct T0
        // * Q1 specifies the value for F(T0)
        // * Q2 invokes F(T0)
        // * Q3 invokes Q1 and then Q2
        // * Q4 invokes Q2 and then Q1
        //
        // Now, if We invoke Q3 first, We get one result for Q2, but if We invoke Q4 first, We get a different value. That's no good.
        let input_key = <C::Input<'db>>::database_key_index(zalsa, key);
        if !zalsa_local.is_tracked_struct_of_active_query(input_key) {
            panic!("can only use `specify` on salsa structs created during the current tracked fn");
        }

        // Subtle: we treat the "input" to a set query as if it were
        // volatile.
        //
        // The idea is this. You have the current query C that
        // created the entity E, and it is setting the value F(E) of the function F.
        // When some other query R reads the field F(E), in order to have obtained
        // the entity E, it has to have executed the query C.
        //
        // This will have forced C to either:
        //
        // - not create E this time, in which case R shouldn't have it (some kind of leak has occurred)
        // - assign a value to F(E), in which case `verified_at` will be the current revision and `changed_at` will be updated appropriately
        // - NOT assign a value to F(E), in which case we need to re-execute the function (which typically panics).
        //
        // So, ruling out the case of a leak having occurred, that means that the reader R will either see:
        //
        // - a result that is verified in the current revision, because it was set, which will use the set value
        // - a result that is NOT verified and has untracked inputs, which will re-execute (and likely panic)

        let revision = zalsa.current_revision();
        let database_key_index = self.database_key_index(key);
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);

        zalsa.unwind_if_revision_cancelled(zalsa_local);
        let _claim_guard =
            match self
                .sync_table
                .try_claim(zalsa, zalsa_local, key, Reentrancy::Deny)
            {
                ClaimResult::Claimed(guard) => guard,
                ClaimResult::Running(_) | ClaimResult::Cycle { .. } => {
                    // The one-shot query is already running and therefore wins this revision.
                    return;
                }
            };

        // Re-read the memo after claiming the query so that no concurrent execution or
        // specification can replace it while we decide whether to keep or overwrite it.
        let old_memo = self.get_memo_from_table_for(zalsa, key, memo_ingredient_index);

        if let Some(old_memo) = old_memo {
            if old_memo.header.verified_at.load() == revision && old_memo.value.is_some() {
                // A value produced by another query wins this revision.
                let QueryOriginRef::Assigned(owner) = old_memo.header.origin() else {
                    return;
                };
                debug_assert_eq!(owner, active_query_key);

                let first_assignment_in_execution = zalsa_local.add_output(database_key_index);
                // Outputs from a prior provisional iteration are seeded into the active query,
                // so they don't count as duplicate calls in this execution.
                if cycle_heads.is_empty()
                    && !old_memo.header.was_cycle_participant()
                    && !first_assignment_in_execution
                {
                    panic!("cannot call `specify` twice for the same key in one query execution");
                }
            }
        }

        let is_provisional = !cycle_heads.is_empty();
        let mut completed_query = CompletedQuery {
            revisions: QueryRevisions {
                changed_at: current_deps.changed_at,
                durability: current_deps.durability,
                origin_and_extra: OriginAndExtra::assigned(active_query_key),
                #[cfg(feature = "accumulator")]
                accumulated_inputs: Default::default(),
                verified_final: AtomicBool::new(!is_provisional),
            },
            stale_tracked_structs: Vec::new(),
        };
        if is_provisional {
            // Assigned memos aren't cycle heads, so only their inherited head stamps matter.
            completed_query
                .revisions
                .set_cycle_heads(cycle_heads, Default::default());
        }

        if let Some(old_memo) = old_memo {
            completed_query
                .stale_tracked_structs
                .extend_from_slice(old_memo.header.revisions.tracked_struct_ids());
            self.backdate_if_appropriate(
                old_memo,
                database_key_index,
                &mut completed_query.revisions,
                &value,
            );
            old_memo
                .header
                .diff_outputs(zalsa, database_key_index, &completed_query);
        }

        let memo = Memo {
            value: Some(value),
            header: MemoHeader {
                verified_at: AtomicRevision::from(revision),
                revisions: completed_query.revisions,
            },
        };

        crate::tracing::debug!(
            "specify: about to add memo {:#?} for key {:?}",
            memo.tracing_debug(),
            key
        );
        self.insert_memo(zalsa, key, memo, memo_ingredient_index);

        // Record that the current query *specified* a value for this cell.
        zalsa_local.add_output(database_key_index);
    }

    /// Invoked when the query `executor` has been validated as having green inputs
    /// and `key` is a value that was specified by `executor`.
    /// Marks `key` as valid in the current revision since if `executor` had re-executed,
    /// it would have specified `key` again.
    pub(super) fn validate_specified_value(
        &self,
        zalsa: &Zalsa,
        executor: DatabaseKeyIndex,
        key: Id,
    ) {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);

        let memo = match self.get_memo_from_table_for(zalsa, key, memo_ingredient_index) {
            Some(m) => m,
            None => return,
        };

        // If we are marking this as validated, it must be a value that was
        // assigned by `executor`.
        match memo.header.origin() {
            QueryOriginRef::Assigned(by_query) => assert_eq!(by_query, executor),
            _ => panic!(
                "expected a query assigned by `{:?}`, not `{:?}`",
                executor,
                memo.header.origin(),
            ),
        }

        let database_key_index = self.database_key_index(key);
        memo.header.mark_as_verified(zalsa, database_key_index);
        #[cfg(feature = "accumulator")]
        memo.header
            .revisions
            .accumulated_inputs
            .store(InputAccumulatedValues::Empty);
    }
}
