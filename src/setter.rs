use crate::id::AsId;
use crate::input::Configuration;
use crate::input_field::{InputFieldData, InputFieldIngredient};
use crate::{Durability, Runtime};
use std::hash::Hash;

#[must_use]
pub struct Setter<'setter, C: Configuration, F: InputFieldData> {
    runtime: &'setter mut Runtime,
    key: C::Id,
    ingredient: &'setter mut InputFieldIngredient<C, F>,
    durability: Durability,
}

impl<'setter, C, F> Setter<'setter, C, F>
where
    C: Configuration,
    F: InputFieldData,
{
    pub fn new(
        runtime: &'setter mut Runtime,
        key: C::Id,
        ingredient: &'setter mut InputFieldIngredient<C, F>,
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
