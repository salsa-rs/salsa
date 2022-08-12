use crate::{
    cycle::CycleRecoveryStrategy,
    ingredient::Ingredient,
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::{local_state::QueryOrigin, Runtime},
    AsId, IngredientIndex, Revision,
};

pub trait InputId: AsId {}
impl<T: AsId> InputId for T {}

pub struct InputIngredient<Id>
where
    Id: InputId,
{
    ingredient_index: IngredientIndex,
    counter: u32,
    _phantom: std::marker::PhantomData<Id>,
}

impl<Id> InputIngredient<Id>
where
    Id: InputId,
{
    pub fn new(index: IngredientIndex) -> Self {
        Self {
            ingredient_index: index,
            counter: Default::default(),
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.ingredient_index,
            key_index: id.as_id(),
        }
    }

    pub fn new_input(&mut self, _runtime: &mut Runtime) -> Id {
        let next_id = self.counter;
        self.counter += 1;
        Id::from_id(crate::Id::from_u32(next_id))
    }
}

impl<DB: ?Sized, Id> Ingredient<DB> for InputIngredient<Id>
where
    Id: InputId,
{
    fn maybe_changed_after(&self, _db: &DB, _input: DependencyIndex, _revision: Revision) -> bool {
        // Input ingredients are just a counter, they store no data, they are immortal.
        // Their *fields* are stored in function ingredients elsewhere.
        false
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn remove_stale_output(
        &self,
        _db: &DB,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "remove_stale_output({:?}, {:?}): input cannot be the output of a tracked function",
            executor, stale_output_key
        );
    }
}
