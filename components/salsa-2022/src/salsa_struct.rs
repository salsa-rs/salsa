use crate::{Database, IngredientIndex};

pub trait SalsaStructInDb<DB: ?Sized + Database> {
    fn register_dependent_fn(db: &DB, index: IngredientIndex);
}
