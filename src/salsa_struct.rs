use crate::{zalsa::IngredientIndex, Database};

pub trait SalsaStructInDb {
    fn register_dependent_fn(db: &dyn Database, index: IngredientIndex);
}

impl SalsaStructInDb for () {
    fn register_dependent_fn(_db: &dyn Database, _index: IngredientIndex) {}
}
