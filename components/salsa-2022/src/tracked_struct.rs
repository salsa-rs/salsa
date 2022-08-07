use crate::{
    cycle::CycleRecoveryStrategy,
    ingredient::{Ingredient, MutIngredient},
    interned::{InternedData, InternedId, InternedIngredient},
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::{local_state::QueryInputs, Runtime},
    Database, IngredientIndex, Revision,
};

pub trait TrackedStructId: InternedId {}
impl<T: InternedId> TrackedStructId for T {}

pub trait TrackedStructData: InternedData {}
impl<T: InternedData> TrackedStructData for T {}

pub trait TrackedStructInDb<DB: ?Sized + Database> {
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
    pub fn new(index: IngredientIndex) -> Self {
        Self {
            interned: InternedIngredient::new(index),
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
        runtime.add_entity_created(self.database_key_index(result));
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
    pub(crate) fn delete_entities(&self, _runtime: &Runtime, ids: impl Iterator<Item = Id>) {
        for id in ids {
            self.interned.delete_index(id);
        }
    }
}

impl<DB: ?Sized, Id, Data> Ingredient<DB> for TrackedStructIngredient<Id, Data>
where
    Id: TrackedStructId,
    Data: TrackedStructData,
{
    fn maybe_changed_after(&self, db: &DB, input: DependencyIndex, revision: Revision) -> bool {
        self.interned.maybe_changed_after(db, input, revision)
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        <_ as Ingredient<DB>>::cycle_recovery_strategy(&self.interned)
    }

    fn inputs(&self, _key_index: crate::Id) -> Option<QueryInputs> {
        None
    }
}

impl<DB: ?Sized, Id, Data> MutIngredient<DB> for TrackedStructIngredient<Id, Data>
where
    Id: TrackedStructId,
    Data: TrackedStructData,
{
    fn reset_for_new_revision(&mut self) {
        self.interned.clear_deleted_indices();
    }
}
