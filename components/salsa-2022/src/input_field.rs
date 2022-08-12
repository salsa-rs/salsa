use crate::cycle::CycleRecoveryStrategy;
use crate::ingredient::Ingredient;
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryEdges;
use crate::runtime::StampedValue;
use crate::{AsId, DatabaseKeyIndex, Durability, Id, IngredientIndex, Revision, Runtime};
use rustc_hash::FxHashMap;
use std::hash::Hash;

/// Ingredient used to represent the fields of a `#[salsa::input]`.
/// These fields can only be mutated by an explicit call to a setter
/// with an `&mut` reference to the database,
/// and therefore cannot be mutated during a tracked function or in parallel.
/// This makes the implementation considerably simpler.
pub struct InputFieldIngredient<K, F> {
    index: IngredientIndex,
    map: FxHashMap<K, StampedValue<F>>,
}

impl<K, F> InputFieldIngredient<K, F>
where
    K: Eq + Hash + AsId,
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
            changed_at: revision,
        };

        if let Some(old_value) = self.map.insert(key, stamped_value) {
            Some(old_value.value)
        } else {
            None
        }
    }

    pub fn fetch(&self, runtime: &Runtime, key: K) -> &F {
        let StampedValue {
            value,
            durability,
            changed_at,
        } = self.map.get(&key).unwrap();

        runtime.report_tracked_read(
            self.database_key_index(key).into(),
            *durability,
            *changed_at,
        );

        value
    }

    fn database_key_index(&self, key: K) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.index,
            key_index: key.as_id(),
        }
    }
}

impl<DB: ?Sized, K, F> Ingredient<DB> for InputFieldIngredient<K, F>
where
    K: AsId,
{
    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn maybe_changed_after(&self, _db: &DB, input: DependencyIndex, revision: Revision) -> bool {
        let key: K = AsId::from_id(input.key_index.unwrap());
        self.map.get(&key).unwrap().changed_at > revision
    }

    fn inputs(&self, _key_index: Id) -> Option<QueryEdges> {
        None
    }

    fn remove_stale_output(&self, _executor: DatabaseKeyIndex, _stale_output_key: Option<Id>) {
        todo!()
    }
}
