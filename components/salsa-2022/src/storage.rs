use std::{fmt, sync::Arc};

use parking_lot::Condvar;

use crate::cycle::CycleRecoveryStrategy;
use crate::ingredient::Ingredient;
use crate::jar::Jar;
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryOrigin;
use crate::runtime::Runtime;
use crate::{Database, DatabaseKeyIndex, Id, IngredientIndex};

use super::routes::Routes;
use super::{ParallelDatabase, Revision};

/// The "storage" struct stores all the data for the jars.
/// It is shared between the main database and any active snapshots.
pub struct Storage<DB: HasJars> {
    /// Data shared across all databases. This contains the ingredients needed by each jar.
    /// See the ["jars and ingredients" chapter](https://salsa-rs.github.io/salsa/plumbing/jars_and_ingredients.html)
    /// for more detailed description.
    ///
    /// Even though this struct is stored in an `Arc`, we sometimes get mutable access to it
    /// by using `Arc::get_mut`. This is only possible when all parallel snapshots have been dropped.
    shared: Arc<Shared<DB>>,

    /// The "ingredients" structure stores the information about how to find each ingredient in the database.
    /// It allows us to take the [`IngredientIndex`] assigned to a particular ingredient
    /// and get back a [`dyn Ingredient`][`Ingredient`] for the struct that stores its data.
    ///
    /// This is kept separate from `shared` so that we can clone it and retain `&`-access even when we have `&mut` access to `shared`.
    routes: Arc<Routes<DB>>,

    /// The runtime for this particular salsa database handle.
    /// Each handle gets its own runtime, but the runtimes have shared state between them.
    runtime: Runtime,
}

/// Data shared between all threads.
/// This is where the actual data for tracked functions, structs, inputs, etc lives,
/// along with some coordination variables between treads.
struct Shared<DB: HasJars> {
    /// Contains the data for each jar in the database.
    /// Each jar stores its own structs in there that ultimately contain ingredients
    /// (types that implement the [`Ingredient`] trait, like [`crate::function::FunctionIngredient`]).
    jars: DB::Jars,

    /// Conditional variable that is used to coordinate cancellation.
    /// When the main thread writes to the database, it blocks until each of the snapshots can be cancelled.
    cvar: Condvar,
}

// ANCHOR: default
impl<DB> Default for Storage<DB>
where
    DB: HasJars,
{
    fn default() -> Self {
        let mut routes = Routes::new();
        let jars = DB::create_jars(&mut routes);
        Self {
            shared: Arc::new(Shared {
                jars,
                cvar: Default::default(),
            }),
            routes: Arc::new(routes),
            runtime: Runtime::default(),
        }
    }
}
// ANCHOR_END: default

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
            routes: self.routes.clone(),
            runtime: self.runtime.snapshot(),
        }
    }

    pub fn jars(&self) -> (&DB::Jars, &Runtime) {
        (&self.shared.jars, &self.runtime)
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime {
        self.jars_mut().1
    }

    // ANCHOR: jars_mut
    /// Gets mutable access to the jars. This will trigger a new revision
    /// and it will also cancel any ongoing work in the current revision.
    /// Any actual writes that occur to data in a jar should use
    /// [`Runtime::report_tracked_write`].
    pub fn jars_mut(&mut self) -> (&mut DB::Jars, &mut Runtime) {
        // Wait for all snapshots to be dropped.
        self.cancel_other_workers();

        // Increment revision counter.
        self.runtime.new_revision();

        // Acquire `&mut` access to `self.shared` -- this is only possible because
        // the snapshots have all been dropped, so we hold the only handle to the `Arc`.
        let shared = Arc::get_mut(&mut self.shared).unwrap();

        // Inform other ingredients that a new revision has begun.
        // This gives them a chance to free resources that were being held until the next revision.
        let routes = self.routes.clone();
        for route in routes.reset_routes() {
            route(&mut shared.jars).reset_for_new_revision();
        }

        // Return mut ref to jars + runtime.
        (&mut shared.jars, &mut self.runtime)
    }
    // ANCHOR_END: jars_mut

    // ANCHOR: cancel_other_workers
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

            // Otherwise, wait until some other storage entities have dropped.
            // We create a mutex here because the cvar api requires it, but we
            // don't really need one as the data being protected is actually
            // the jars above.
            //
            // The cvar `self.shared.cvar` is notified by the `Drop` impl.
            let mutex = parking_lot::Mutex::new(());
            let mut guard = mutex.lock();
            self.shared.cvar.wait(&mut guard);
        }
    }
    // ANCHOR_END: cancel_other_workers

    pub fn ingredient(&self, ingredient_index: IngredientIndex) -> &dyn Ingredient<DB> {
        let route = self.routes.route(ingredient_index);
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

    fn create_jars(routes: &mut Routes<Self>) -> Self::Jars;
}

pub trait DbWithJar<J>: HasJar<J> + Database {
    fn as_jar_db<'db>(&'db self) -> &<J as Jar<'db>>::DynDb
    where
        J: Jar<'db>;
}

pub trait JarFromJars<J>: HasJars {
    fn jar_from_jars(jars: &Self::Jars) -> &J;

    fn jar_from_jars_mut(jars: &mut Self::Jars) -> &mut J;
}

pub trait HasJar<J> {
    fn jar(&self) -> (&J, &Runtime);

    fn jar_mut(&mut self) -> (&mut J, &mut Runtime);
}

// ANCHOR: HasJarsDyn
/// Dyn friendly subset of HasJars
pub trait HasJarsDyn {
    fn runtime(&self) -> &Runtime;

    fn runtime_mut(&mut self) -> &mut Runtime;

    fn maybe_changed_after(&self, input: DependencyIndex, revision: Revision) -> bool;

    fn cycle_recovery_strategy(&self, input: IngredientIndex) -> CycleRecoveryStrategy;

    fn origin(&self, input: DatabaseKeyIndex) -> Option<QueryOrigin>;

    fn mark_validated_output(&self, executor: DatabaseKeyIndex, output: DependencyIndex);

    /// Invoked when `executor` used to output `stale_output` but no longer does.
    /// This method routes that into a call to the [`remove_stale_output`](`crate::ingredient::Ingredient::remove_stale_output`)
    /// method on the ingredient for `stale_output`.
    fn remove_stale_output(&self, executor: DatabaseKeyIndex, stale_output: DependencyIndex);

    /// Informs `ingredient` that the salsa struct with id `id` has been deleted.
    /// This means that `id` will not be used in this revision and hence
    /// any memoized values keyed by that struct can be discarded.
    ///
    /// In order to receive this callback, `ingredient` must have registered itself
    /// as a dependent function using
    /// [`SalsaStructInDb::register_dependent_fn`](`crate::salsa_struct::SalsaStructInDb::register_dependent_fn`).
    fn salsa_struct_deleted(&self, ingredient: IngredientIndex, id: Id);

    fn fmt_index(&self, index: DependencyIndex, fmt: &mut fmt::Formatter<'_>) -> fmt::Result;
}
// ANCHOR_END: HasJarsDyn

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

    fn create_ingredients<DB>(routes: &mut Routes<DB>) -> Self::Ingredients
    where
        DB: DbWithJar<Self::Jar> + JarFromJars<Self::Jar>;
}
