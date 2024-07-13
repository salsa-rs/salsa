use std::marker::PhantomData;

use crate::input::{Configuration, IngredientImpl};
use crate::{Durability, Runtime};

#[must_use]
pub struct Setter<'setter, C: Configuration, S, F> {
    runtime: &'setter mut Runtime,
    id: C::Id,
    ingredient: &'setter mut IngredientImpl<C>,
    durability: Durability,
    field_index: usize,
    setter: S,
    phantom: PhantomData<fn(F)>,
}

impl<'setter, C, S, F> Setter<'setter, C, S, F>
where
    C: Configuration,
    S: FnOnce(&mut C::Fields, F) -> F,
{
    pub fn new(
        runtime: &'setter mut Runtime,
        id: C::Id,
        field_index: usize,
        ingredient: &'setter mut IngredientImpl<C>,
        setter: S,
    ) -> Self {
        Setter {
            runtime,
            id,
            field_index,
            ingredient,
            durability: Durability::LOW,
            setter,
            phantom: PhantomData,
        }
    }

    pub fn with_durability(self, durability: Durability) -> Self {
        Setter { durability, ..self }
    }

    pub fn to(self, value: F) -> F {
        let Self {
            runtime,
            id,
            ingredient,
            durability,
            field_index,
            setter,
            phantom: _,
        } = self;

        ingredient.set_field(runtime, id, field_index, durability, |tuple| {
            setter(tuple, value)
        })
    }
}
