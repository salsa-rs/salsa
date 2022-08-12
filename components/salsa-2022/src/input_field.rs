use std::hash::Hash;
use rustc_hash::FxHashMap;
use crate::{Durability, IngredientIndex, Revision, Runtime};
use crate::cycle::CycleRecoveryStrategy;
use crate::ingredient::Ingredient;
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryInputs;
use crate::runtime::StampedValue;

/// Ingredient used to represent the fields of a `#[salsa::input]`.
/// These fields can only be mutated by an explicit call to a setter
/// with an `&mut` reference to the database,
/// and therefore cannot be mutated during a tracked function or in parallel.
/// This makes the implementation considerably simpler.
pub struct InputFieldIngredient<K, F> {
    index: IngredientIndex,
    map: FxHashMap<K, StampedValue<F>>
}

impl<K, F> InputFieldIngredient<K, F>
where K: Eq + Hash
{
    pub fn new(index: IngredientIndex) -> Self {
        Self {
            index,
            map: Default::default(),
        }
    }

    pub fn store(
        &mut self,
        runtime: &mut Runtime,
        key: K,
        value: F,
        durability: Durability,
    ) -> Option<F> {
        let revision = runtime.current_revision();
        let stamped_value = StampedValue {
            value,
            durability,
            changed_at: revision
        };

        if let Some(old_value) = self.map.insert(key, stamped_value) {
            Some(old_value.value)
        } else {
            None
        }
    }

    pub fn fetch(
        &self,
        key: K,
    ) -> &F {
        &self.map.get(&key).unwrap().value
    }
}

impl<DB: ?Sized, K, F> Ingredient<DB> for InputFieldIngredient<K, F>
{
    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn maybe_changed_after(&self, _db: &DB, _input: DependencyIndex, _revision: Revision) -> bool {
        false
    }

    fn inputs(&self, _key_index: crate::Id) -> Option<QueryInputs> {
        None
    }
}
