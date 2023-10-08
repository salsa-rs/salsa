use std::fmt;

use crate::{
    cycle::CycleRecoveryStrategy,
    ingredient::{fmt_index, Ingredient, IngredientRequiresReset},
    ingredient_list::IngredientList,
    interned::{InternedData, InternedId, InternedIngredient},
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::{local_state::QueryOrigin, Runtime},
    salsa_struct::SalsaStructInDb,
    Database, Event, IngredientIndex, Revision,
};

pub trait TrackedStructId: InternedId {}
impl<T: InternedId> TrackedStructId for T {}

pub trait TrackedStructData: InternedData {}
impl<T: InternedData> TrackedStructData for T {}

pub trait TrackedStructInDb<DB: ?Sized + Database>: SalsaStructInDb<DB> {
    /// Converts the identifier for this tracked struct into a `DatabaseKeyIndex`.
    fn database_key_index(self, db: &DB) -> DatabaseKeyIndex;
}

/// Created for each tracked struct.
/// This ingredient only stores the "id" fields.
/// It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked struct id.
/// The value fields are stored in [`crate::function::FunctionIngredient`] instances keyed by the tracked struct id.
/// Unlike normal interners, tracked struct indices can be deleted and reused aggressively:
/// when a tracked function re-executes,
/// any tracked structs that it created before but did not create this time can be deleted.
pub struct TrackedStructIngredient<Id, Data>
where
    Id: TrackedStructId,
    Data: TrackedStructData,
{
    interned: InternedIngredient<Id, TrackedStructKey<Data>>,

    /// A list of each tracked function `f` whose key is this
    /// tracked struct.
    ///
    /// Whenever an instance `i` of this struct is deleted,
    /// each of these functions will be notified
    /// so they can remove any data tied to that instance.
    dependent_fns: IngredientList,

    debug_name: &'static str,
}

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Copy, Clone)]
struct TrackedStructKey<Data> {
    query_key: Option<DatabaseKeyIndex>,
    disambiguator: Disambiguator,
    data: Data,
}

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Copy, Clone)]
pub struct Disambiguator(pub u32);

impl<Id, Data> TrackedStructIngredient<Id, Data>
where
    Id: TrackedStructId,
    Data: TrackedStructData,
{
    pub fn new(index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            interned: InternedIngredient::new(index, debug_name),
            dependent_fns: IngredientList::new(),
            debug_name,
        }
    }

    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.interned.ingredient_index(),
            key_index: id.as_id(),
        }
    }

    pub fn new_struct(&self, runtime: &Runtime, data: Data) -> Id {
        let data_hash = crate::hash::hash(&data);
        let (query_key, disambiguator) = runtime.disambiguate_entity(
            self.interned.ingredient_index(),
            self.interned.reset_at(),
            data_hash,
        );
        let entity_key = TrackedStructKey {
            query_key: Some(query_key),
            disambiguator,
            data,
        };
        let result = self.interned.intern(runtime, entity_key);
        runtime.add_output(self.database_key_index(result).into());
        result
    }

    pub fn tracked_struct_data<'db>(&'db self, runtime: &'db Runtime, id: Id) -> &'db Data {
        &self.interned.data(runtime, id).data
    }

    /// Deletes the given entities. This is used after a query `Q` executes and we can compare
    /// the entities `E_now` that it produced in this revision vs the entities
    /// `E_prev` it produced in the last revision. Any missing entities `E_prev - E_new` can be
    /// deleted.
    ///
    /// # Warning
    ///
    /// Using this method on an entity id that MAY be used in the current revision will lead to
    /// unspecified results (but not UB). See [`InternedIngredient::delete_index`] for more
    /// discussion and important considerations.
    pub(crate) fn delete_entity(&self, db: &dyn crate::Database, id: Id) {
        db.salsa_event(Event {
            runtime_id: db.runtime().id(),
            kind: crate::EventKind::DidDiscard {
                key: self.database_key_index(id),
            },
        });

        self.interned.delete_index(id);
        for dependent_fn in self.dependent_fns.iter() {
            db.salsa_struct_deleted(dependent_fn, id.as_id());
        }
    }

    /// Adds a dependent function (one keyed by this tracked struct) to our list.
    /// When instances of this struct are deleted, these dependent functions
    /// will be notified.
    pub fn register_dependent_fn(&self, index: IngredientIndex) {
        self.dependent_fns.push(index);
    }
}

impl<DB: ?Sized, Id, Data> Ingredient<DB> for TrackedStructIngredient<Id, Data>
where
    Id: TrackedStructId,
    Data: TrackedStructData,
    DB: crate::Database,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.interned.ingredient_index()
    }

    fn maybe_changed_after(&self, db: &DB, input: DependencyIndex, revision: Revision) -> bool {
        self.interned.maybe_changed_after(db, input, revision)
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        <_ as Ingredient<DB>>::cycle_recovery_strategy(&self.interned)
    }

    fn origin(&self, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &DB,
        _executor: DatabaseKeyIndex,
        _output_key: Option<crate::Id>,
    ) {
        // FIXME
    }

    fn remove_stale_output(
        &self,
        db: &DB,
        _executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        // This method is called when, in prior revisions,
        // `executor` creates a tracked struct `salsa_output_key`,
        // but it did not in the current revision.
        // In that case, we can delete `stale_output_key` and any data associated with it.
        let stale_output_key: Id = Id::from_id(stale_output_key.unwrap());
        self.delete_entity(db.as_salsa_database(), stale_output_key);
    }

    fn reset_for_new_revision(&mut self) {
        self.interned.clear_deleted_indices();
    }

    fn salsa_struct_deleted(&self, _db: &DB, _id: crate::Id) {
        panic!("unexpected call: interned ingredients do not register for salsa struct deletion events");
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(self.debug_name, index, fmt)
    }
}

impl<Id, Data> IngredientRequiresReset for TrackedStructIngredient<Id, Data>
where
    Id: TrackedStructId,
    Data: TrackedStructData,
{
    const RESET_ON_NEW_REVISION: bool = true;
}
