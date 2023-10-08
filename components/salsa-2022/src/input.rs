use std::{
    fmt,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    cycle::CycleRecoveryStrategy,
    ingredient::{fmt_index, Ingredient, IngredientRequiresReset},
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
    counter: AtomicU32,
    debug_name: &'static str,
    _phantom: std::marker::PhantomData<Id>,
}

impl<Id> InputIngredient<Id>
where
    Id: InputId,
{
    pub fn new(index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            ingredient_index: index,
            counter: Default::default(),
            debug_name,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.ingredient_index,
            key_index: id.as_id(),
        }
    }

    pub fn new_input(&self, _runtime: &Runtime) -> Id {
        let next_id = self.counter.fetch_add(1, Ordering::Relaxed);
        Id::from_id(crate::Id::from_u32(next_id))
    }

    pub fn new_singleton_input(&self, _runtime: &Runtime) -> Id {
        // when one exists already, panic
        if self.counter.load(Ordering::Relaxed) >= 1 {
            panic!("singleton struct may not be duplicated");
        }
        // fresh new ingredient
        self.counter.store(1, Ordering::Relaxed);
        Id::from_id(crate::Id::from_u32(0))
    }

    pub fn get_singleton_input(&self, _runtime: &Runtime) -> Option<Id> {
        (self.counter.load(Ordering::Relaxed) > 0).then(|| Id::from_id(crate::Id::from_u32(0)))
    }
}

impl<DB: ?Sized, Id> Ingredient<DB> for InputIngredient<Id>
where
    Id: InputId,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

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

    fn mark_validated_output(
        &self,
        _db: &DB,
        executor: DatabaseKeyIndex,
        output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "mark_validated_output({:?}, {:?}): input cannot be the output of a tracked function",
            executor, output_key
        );
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

    fn reset_for_new_revision(&mut self) {
        panic!("unexpected call to `reset_for_new_revision`")
    }

    fn salsa_struct_deleted(&self, _db: &DB, _id: crate::Id) {
        panic!(
            "unexpected call: input ingredients do not register for salsa struct deletion events"
        );
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(self.debug_name, index, fmt)
    }
}

impl<Id> IngredientRequiresReset for InputIngredient<Id>
where
    Id: InputId,
{
    const RESET_ON_NEW_REVISION: bool = false;
}
