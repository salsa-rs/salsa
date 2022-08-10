use crossbeam::atomic::AtomicCell;

use crate::{
    runtime::local_state::{QueryInputs, QueryRevisions},
    tracked_struct::TrackedStructInDb,
    Database,
};

use super::{memo::Memo, Configuration, DynDb, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// Specifies the value of the function for the given key.
    /// This is a way to imperatively set the value of a function.
    /// It only works if the key is a tracked struct created in the current query.
    pub fn specify<'db>(&self, db: &'db DynDb<'db, C>, key: C::Key, value: C::Value)
    where
        C::Key: TrackedStructInDb<DynDb<'db, C>>,
    {
        let runtime = db.salsa_runtime();

        let (_, current_deps) = match runtime.active_query() {
            Some(v) => v,
            None => panic!("can only use `set` with an active query"),
        };

        let entity_index = key.database_key_index(db);
        if !runtime.was_entity_created(entity_index) {
            panic!("can only use `set` on entities created during current query");
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
        let inputs = QueryInputs {
            untracked: false,
            tracked: runtime.empty_dependencies(),
        };

        let revision = runtime.current_revision();
        let mut revisions = QueryRevisions {
            changed_at: current_deps.changed_at,
            durability: current_deps.durability,
            inputs,
        };

        if let Some(old_memo) = self.memo_map.get(key) {
            self.backdate_if_appropriate(&old_memo, &mut revisions, &value);
        }

        let memo = Memo {
            value: Some(value),
            verified_at: AtomicCell::new(revision),
            revisions,
        };

        self.insert_memo(key, memo);
    }
}
