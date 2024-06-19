use crate::{Database, IngredientIndex};

pub trait SalsaStructInDb<DB: ?Sized + Database> {
    fn register_dependent_fn(db: &DB, index: IngredientIndex);
}

/// A ZST that implements [`SalsaStructInDb`]
///
/// It is used for implementing "constant" tracked function
/// (ones that only take a database as an argument).
pub struct Singleton;

impl<DB: ?Sized + Database> SalsaStructInDb<DB> for Singleton {
    fn register_dependent_fn(_db: &DB, _index: IngredientIndex) {}
}
