use std::sync::atomic::AtomicBool;

use crate::{
    accumulator::accumulated_map::InputAccumulatedValues,
    revision::AtomicRevision,
    tracked_struct::TrackedStructInDb,
    zalsa::ZalsaDatabase,
    zalsa_local::{QueryOrigin, QueryRevisions},
    AsDynDatabase as _, Database, DatabaseKeyIndex, Id,
};

use super::{memo::Memo, Configuration, IngredientImpl};

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

        let (active_query_key, current_deps) = match zalsa_local.active_query() {
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
        let database_key_index = <C::Input<'db>>::database_key_index(db.as_dyn_database(), key);
        if !zalsa_local.is_output_of_active_query(database_key_index) {
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

        let revision = db.zalsa().current_revision();
        let mut revisions = QueryRevisions {
            changed_at: current_deps.changed_at,
            durability: current_deps.durability,
            origin: QueryOrigin::Assigned(active_query_key),
            tracked_struct_ids: Default::default(),
            accumulated: Default::default(),
            accumulated_inputs: Default::default(),
            verified_final: AtomicBool::new(true),
            cycle_heads: Default::default(),
        };

        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);
        if let Some(old_memo) = self.get_memo_from_table_for(zalsa, key, memo_ingredient_index) {
            self.backdate_if_appropriate(old_memo, &mut revisions, &value);
            self.diff_outputs(
                zalsa,
                db,
                database_key_index,
                old_memo,
                &mut revisions,
                false,
            );
        }

        let memo = Memo {
            value: Some(value),
            verified_at: AtomicRevision::from(revision),
            revisions,
        };

        tracing::debug!(
            "specify: about to add memo {:#?} for key {:?}",
            memo.tracing_debug(),
            key
        );
        self.insert_memo(zalsa, key, memo, memo_ingredient_index);

        // Record that the current query *specified* a value for this cell.
        let database_key_index = self.database_key_index(key);
        zalsa_local.add_output(database_key_index);
    }

    /// Invoked when the query `executor` has been validated as having green inputs
    /// and `key` is a value that was specified by `executor`.
    /// Marks `key` as valid in the current revision since if `executor` had re-executed,
    /// it would have specified `key` again.
    pub(super) fn validate_specified_value<Db: ?Sized + Database>(
        &self,
        db: &Db,
        executor: DatabaseKeyIndex,
        key: Id,
    ) {
        let zalsa = db.zalsa();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);

        let memo = match self.get_memo_from_table_for(zalsa, key, memo_ingredient_index) {
            Some(m) => m,
            None => return,
        };

        // If we are marking this as validated, it must be a value that was
        // assigneed by `executor`.
        match memo.revisions.origin {
            QueryOrigin::Assigned(by_query) => assert_eq!(by_query, executor),
            _ => panic!(
                "expected a query assigned by `{:?}`, not `{:?}`",
                executor, memo.revisions.origin,
            ),
        }

        let database_key_index = self.database_key_index(key);
        memo.mark_as_verified(
            db,
            zalsa.current_revision(),
            database_key_index,
            InputAccumulatedValues::Empty,
        );
    }
}
