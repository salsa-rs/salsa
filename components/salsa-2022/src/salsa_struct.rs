use crate::{Database, IngredientIndex};

pub trait SalsaStructInDb<DB: ?Sized + Database> {
    fn register_dependent_fn(db: &DB, index: IngredientIndex);
}

impl<DB: ?Sized + Database> SalsaStructInDb<DB> for () {
    fn register_dependent_fn(_db: &DB, _index: IngredientIndex) {}
}
