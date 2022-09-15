use crate::input_field::InputFieldIngredient;
use crate::{AsId, Durability, Runtime};
use std::hash::Hash;

#[must_use]
pub struct Setter<'setter, K, F> {
    runtime: &'setter mut Runtime,
    key: K,
    ingredient: &'setter mut InputFieldIngredient<K, F>,
    durability: Durability,
}

impl<'setter, K, F> Setter<'setter, K, F>
where
    K: Eq + Hash + AsId,
{
    pub fn new(
        runtime: &'setter mut Runtime,
        key: K,
        ingredient: &'setter mut InputFieldIngredient<K, F>,
    ) -> Self {
        Setter {
            runtime,
            key,
            ingredient,
            durability: Durability::LOW,
        }
    }

    pub fn with_durability(self, durability: Durability) -> Self {
        Setter { durability, ..self }
    }

    pub fn to(self, value: F) -> F {
        self.ingredient
            .store_mut(self.runtime, self.key, value, self.durability)
            .unwrap()
    }
}
