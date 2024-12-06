use crate::{plumbing::JarAux, IngredientIndex};

pub trait SalsaStructInDb {
    fn lookup_ingredient_index(aux: &dyn JarAux) -> Option<IngredientIndex>;
}
