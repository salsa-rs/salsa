use crossbeam::atomic::AtomicCell;

use crate::{
    database::AsSalsaDatabase,
    runtime::local_state::{QueryOrigin, QueryRevisions},
    storage::HasJarsDyn,
    tracked_struct::TrackedStructInDb,
    DatabaseKeyIndex, DebugWithDb,
};

use super::{memo::Memo, Configuration, DynDb, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// Specifies the value of the function for the given key.
    /// This is a way to imperatively set the value of a function.
    /// It only works if the key is a tracked struct created in the current query.
    pub(crate) fn specify<'db>(
        &self,
        db: &'db DynDb<'db, C>,
        key: C::Key,
        value: C::Value,
        origin: impl Fn(DatabaseKeyIndex) -> QueryOrigin,
    ) where
        C::Key: TrackedStructInDb<DynDb<'db, C>>,
    {
        let runtime = db.runtime();

        let (active_query_key, current_deps) = match runtime.active_query() {
            Some(v) => v,
            None => panic!("can only use `specify` with an active query"),
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
        let database_key_index = key.database_key_index(db);
        let dependency_index = database_key_index.into();
        if !runtime.is_output_of_active_query(dependency_index) {
            panic!("can only use `specfiy` on entities created during current query");
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

        let revision = runtime.current_revision();
        let mut revisions = QueryRevisions {
            changed_at: current_deps.changed_at,
            durability: current_deps.durability,
            origin: origin(active_query_key),
        };

        if let Some(old_memo) = self.memo_map.get(key) {
            self.backdate_if_appropriate(&old_memo, &mut revisions, &value);
            self.diff_outputs(db, database_key_index, &old_memo, &revisions);
        }

        let memo = Memo {
            value: Some(value),
            verified_at: AtomicCell::new(revision),
            revisions,
        };

        log::debug!("specify: about to add memo {:#?} for key {:?}", memo, key);
        self.insert_memo(db, key, memo);
    }

    /// Specify the value for `key` *and* record that we did so.
    /// Used for explicit calls to `specify`, but not needed for pre-declared tracked struct fields.
    pub fn specify_and_record<'db>(&self, db: &'db DynDb<'db, C>, key: C::Key, value: C::Value)
    where
        C::Key: TrackedStructInDb<DynDb<'db, C>>,
    {
        self.specify(db, key, value, |database_key_index| {
            QueryOrigin::Assigned(database_key_index)
        });

        // Record that the current query *specified* a value for this cell.
        let database_key_index = self.database_key_index(key);
        db.runtime().add_output(database_key_index.into());
    }

    /// Invoked when the query `executor` has been validated as having green inputs
    /// and `key` is a value that was specified by `executor`.
    /// Marks `key` as valid in the current revision since if `executor` had re-executed,
    /// it would have specified `key` again.
    pub(super) fn validate_specified_value(
        &self,
        db: &DynDb<'_, C>,
        executor: DatabaseKeyIndex,
        key: C::Key,
    ) {
        let runtime = db.runtime();

        let memo = match self.memo_map.get(key) {
            Some(m) => m,
            None => return,
        };

        // If we are marking this as validated, it must be a value that was
        // assigneed by `executor`.
        match memo.revisions.origin {
            QueryOrigin::Assigned(by_query) => assert_eq!(by_query, executor),
            _ => panic!(
                "expected a query assigned by `{:?}`, not `{:?}`",
                executor.debug(db),
                memo.revisions.origin,
            ),
        }

        let database_key_index = self.database_key_index(key);
        memo.mark_as_verified(db.as_salsa_database(), runtime, database_key_index);
    }
}
