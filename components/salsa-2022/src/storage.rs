use std::sync::Arc;

use parking_lot::Condvar;

use crate::cycle::CycleRecoveryStrategy;
use crate::ingredient::Ingredient;
use crate::jar::Jar;
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryInputs;
use crate::runtime::Runtime;
use crate::{Database, DatabaseKeyIndex, IngredientIndex};

use super::routes::Ingredients;
use super::{ParallelDatabase, Revision};

#[allow(dead_code)]
pub struct Storage<DB: HasJars> {
    shared: Arc<Shared<DB>>,
    ingredients: Arc<Ingredients<DB>>,
    runtime: Runtime,
}

struct Shared<DB: HasJars> {
    jars: DB::Jars,
    cvar: Condvar,
}

impl<DB> Default for Storage<DB>
where
    DB: HasJars,
{
    fn default() -> Self {
        let mut ingredients = Ingredients::new();
        let jars = DB::create_jars(&mut ingredients);
        Self {
            shared: Arc::new(Shared {
                jars,
                cvar: Default::default(),
            }),
            ingredients: Arc::new(ingredients),
            runtime: Runtime::default(),
        }
    }
}

impl<DB> Storage<DB>
where
    DB: HasJars,
{
    pub fn snapshot(&self) -> Storage<DB>
    where
        DB: ParallelDatabase,
    {
        Self {
            shared: self.shared.clone(),
            ingredients: self.ingredients.clone(),
            runtime: self.runtime.snapshot(),
        }
    }

    pub fn jars(&self) -> (&DB::Jars, &Runtime) {
        (&self.shared.jars, &self.runtime)
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Gets mutable access to the jars. This will trigger a new revision
    /// and it will also cancel any ongoing work in the current revision.
    /// Any actual writes that occur to data in a jar should use
    /// [`Runtime::report_tracked_write`].
    pub fn jars_mut(&mut self) -> (&mut DB::Jars, &mut Runtime) {
        self.cancel_other_workers();
        self.runtime.new_revision();

        let ingredients = self.ingredients.clone();
        let shared = Arc::get_mut(&mut self.shared).unwrap();
        for route in ingredients.mut_routes() {
            route(&mut shared.jars).reset_for_new_revision();
        }

        (&mut shared.jars, &mut self.runtime)
    }

    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_other_workers(&mut self) {
        loop {
            self.runtime.set_cancellation_flag();

            // If we have unique access to the jars, we are done.
            if Arc::get_mut(&mut self.shared).is_some() {
                return;
            }

            // Otherwise, wait until some other storage entites have dropped.
            // We create a mutex here because the cvar api requires it, but we
            // don't really need one as the data being protected is actually
            // the jars above.
            let mutex = parking_lot::Mutex::new(());
            let mut guard = mutex.lock();
            self.shared.cvar.wait(&mut guard);
        }
    }

    pub fn ingredient(&self, ingredient_index: IngredientIndex) -> &dyn Ingredient<DB> {
        let route = self.ingredients.route(ingredient_index);
        route(&self.shared.jars)
    }
}

impl<DB> Drop for Shared<DB>
where
    DB: HasJars,
{
    fn drop(&mut self) {
        self.cvar.notify_all();
    }
}

pub trait HasJars: HasJarsDyn + Sized {
    type Jars;

    fn jars(&self) -> (&Self::Jars, &Runtime);

    /// Gets mutable access to the jars. This will trigger a new revision
    /// and it will also cancel any ongoing work in the current revision.
    fn jars_mut(&mut self) -> (&mut Self::Jars, &mut Runtime);

    fn create_jars(ingredients: &mut Ingredients<Self>) -> Self::Jars;
}

pub trait DbWithJar<J>: HasJar<J> + Database {
    fn as_jar_db<'db>(&'db self) -> &<J as Jar<'db>>::DynDb
    where
        J: Jar<'db>;
}

pub trait JarFromJars<J>: HasJars {
    fn jar_from_jars<'db>(jars: &Self::Jars) -> &J;

    fn jar_from_jars_mut<'db>(jars: &mut Self::Jars) -> &mut J;
}

pub trait HasJar<J> {
    fn jar(&self) -> (&J, &Runtime);

    fn jar_mut(&mut self) -> (&mut J, &mut Runtime);
}

// Dyn friendly subset of HasJars
pub trait HasJarsDyn {
    fn runtime(&self) -> &Runtime;

    fn maybe_changed_after(&self, input: DependencyIndex, revision: Revision) -> bool;

    fn cycle_recovery_strategy(&self, input: IngredientIndex) -> CycleRecoveryStrategy;

    fn inputs(&self, input: DatabaseKeyIndex) -> Option<QueryInputs>;
}

pub trait HasIngredientsFor<I>
where
    I: IngredientsFor,
{
    fn ingredient(&self) -> &I::Ingredients;
    fn ingredient_mut(&mut self) -> &mut I::Ingredients;
}

pub trait IngredientsFor {
    type Jar;
    type Ingredients;

    fn create_ingredients<DB>(ingredients: &mut Ingredients<DB>) -> Self::Ingredients
    where
        DB: DbWithJar<Self::Jar> + JarFromJars<Self::Jar>;
}
