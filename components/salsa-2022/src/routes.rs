use super::{
    ingredient::{Ingredient, MutIngredient},
    storage::HasJars,
};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IngredientIndex(u32);

impl IngredientIndex {
    fn from(v: usize) -> Self {
        assert!(v < (std::u32::MAX as usize));
        Self(v as u32)
    }

    fn as_usize(self) -> usize {
        self.0 as usize
    }
}

#[allow(type_alias_bounds)]
#[allow(unused_parens)]
pub type DynRoute<DB: HasJars> = dyn Fn(&DB::Jars) -> (&dyn Ingredient<DB>) + Send + Sync;

#[allow(type_alias_bounds)]
#[allow(unused_parens)]
pub type DynMutRoute<DB: HasJars> =
    dyn Fn(&mut DB::Jars) -> (&mut dyn MutIngredient<DB>) + Send + Sync;

pub struct Ingredients<DB: HasJars> {
    routes: Vec<Box<DynRoute<DB>>>,

    // This is NOT indexed by ingredient index. It's just a vector to iterate over.
    mut_routes: Vec<Box<DynMutRoute<DB>>>,
}

impl<DB: HasJars> Ingredients<DB> {
    pub(super) fn new() -> Self {
        Ingredients {
            routes: vec![],
            mut_routes: vec![],
        }
    }

    /// Adds a new ingredient into the ingredients table, returning
    /// the `IngredientIndex` that can be used in a `DatabaseKeyIndex`.
    /// This index can then be used to fetch the "route" so that we can
    /// dispatch calls to `maybe_changed_after`.
    ///
    /// # Parameters
    ///
    /// * `route` -- a closure which, given a database, will identify the ingredient.
    ///   This closure will be invoked to dispatch calls to `maybe_changed_after`.
    /// * `mut_route` -- an optional closure which identifies the ingredient in a mut
    ///   database.
    pub fn push(
        &mut self,
        route: impl (Fn(&DB::Jars) -> &dyn Ingredient<DB>) + Send + Sync + 'static,
    ) -> IngredientIndex {
        let len = self.routes.len();
        self.routes.push(Box::new(route));
        let index = IngredientIndex::from(len);
        index
    }

    /// As [`Self::push`] but for an ingredient that wants a callback whenever
    /// a new revision is published. T+ Send + Synchese callbacks are used to clear out old data.
    pub fn push_mut(
        &mut self,
        route: impl (Fn(&DB::Jars) -> &dyn Ingredient<DB>) + Send + Sync + 'static,
        mut_route: impl (Fn(&mut DB::Jars) -> &mut dyn MutIngredient<DB>) + Send + Sync + 'static,
    ) -> IngredientIndex {
        let index = self.push(route);
        self.mut_routes.push(Box::new(mut_route));
        index
    }

    pub fn route(&self, index: IngredientIndex) -> &dyn Fn(&DB::Jars) -> &dyn Ingredient<DB> {
        &self.routes[index.as_usize()]
    }

    pub fn mut_routes(
        &self,
    ) -> impl Iterator<Item = &dyn Fn(&mut DB::Jars) -> &mut dyn MutIngredient<DB>> + '_ {
        self.mut_routes
            .iter()
            .map(|b| &**b as &dyn Fn(&mut DB::Jars) -> &mut dyn MutIngredient<DB>)
    }
}
