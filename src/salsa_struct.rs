use crate::{storage::IngredientIndex, Database};

pub trait SalsaStructInDb {
    fn register_dependent_fn(db: &dyn Database, index: IngredientIndex);
}

/// A ZST that implements [`SalsaStructInDb`]
///
/// It is used for implementing "constant" tracked function
/// (ones that only take a database as an argument).
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct Singleton;

impl SalsaStructInDb for Singleton {
    fn register_dependent_fn(_db: &dyn Database, _index: IngredientIndex) {}
}
