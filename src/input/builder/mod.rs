use super::{Configuration, IngredientImpl};
use crate::plumbing::Array;
use crate::runtime::Stamp;
use crate::{Durability, Revision};

pub struct BuilderImpl<'builder, C>
where
    C: Configuration,
{
    stamps: C::Stamps,

    ingredient: &'builder IngredientImpl<C>,
}

impl<'builder, const N: usize, C> BuilderImpl<'builder, C>
where
    C: Configuration<Stamps = Array<Stamp, N>>,
{
    pub fn new(revision: Revision, ingredient: &'builder IngredientImpl<C>) -> Self {
        Self {
            ingredient,
            stamps: Array::new([crate::plumbing::stamp(revision, Durability::default()); N]),
        }
    }

    /// Sets the durability of a specific field.
    pub fn set_field_durability(&mut self, index: usize, durability: Durability) -> &mut Self {
        self.stamps[index].durability = durability;
        self
    }

    pub fn durability(&mut self, durability: Durability) {
        for stamp in &mut *self.stamps {
            stamp.durability = durability;
        }
    }

    pub fn build(self, fields: C::Fields) -> C::Struct {
        self.ingredient.new_input(fields, self.stamps)
    }
}
