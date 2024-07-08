use std::any::{Any, TypeId};
use std::ptr::NonNull;
use std::{fmt, sync::Arc};

use append_only_vec::AppendOnlyVec;
use crossbeam::atomic::AtomicCell;
use parking_lot::{Condvar, Mutex};
use rustc_hash::FxHashMap;

use crate::cycle::CycleRecoveryStrategy;
use crate::ingredient::adaptor::AdaptedIngredient;
use crate::ingredient::{Ingredient, Jar, RawIngredient};
use crate::key::DependencyIndex;
use crate::nonce::{Nonce, NonceGenerator};
use crate::runtime::local_state::QueryOrigin;
use crate::runtime::Runtime;
use crate::{Database, DatabaseKeyIndex, DatabaseView, Id};

use super::{ParallelDatabase, Revision};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorageNonce;
static NONCE: NonceGenerator<StorageNonce> = NonceGenerator::new();

/// An ingredient index identifies a particular [`Ingredient`] in the database.
/// The database contains a number of jars, and each jar contains a number of ingredients.
/// Each ingredient is given a unique index as the database is being created.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IngredientIndex(u32);

impl IngredientIndex {
    /// Create an ingredient index from a usize.
    pub(crate) fn from(v: usize) -> Self {
        assert!(v < (u32::MAX as usize));
        Self(v as u32)
    }

    pub(crate) fn as_u32(self) -> u32 {
        self.0
    }

    /// Convert the ingredient index back into a usize.
    pub(crate) fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl std::ops::Add<u32> for IngredientIndex {
    type Output = IngredientIndex;

    fn add(self, rhs: u32) -> Self::Output {
        IngredientIndex(self.0.checked_add(rhs).unwrap())
    }
}

/// The "storage" struct stores all the data for the jars.
/// It is shared between the main database and any active snapshots.
pub struct Storage<Db: Database> {
    /// Data shared across all databases. This contains the ingredients needed by each jar.
    /// See the ["jars and ingredients" chapter](https://salsa-rs.github.io/salsa/plumbing/jars_and_ingredients.html)
    /// for more detailed description.
    shared: Shared<Db>,

    /// The runtime for this particular salsa database handle.
    /// Each handle gets its own runtime, but the runtimes have shared state between them.
    runtime: Runtime,
}

/// Data shared between all threads.
/// This is where the actual data for tracked functions, structs, inputs, etc lives,
/// along with some coordination variables between treads.
struct Shared<Db: Database> {
    nonce: Nonce<StorageNonce>,

    /// Map from the type-id of an `impl Jar` to the index of its first ingredient.
    /// This is using a `Mutex<FxHashMap>` (versus, say, a `FxDashMap`)
    /// so that we can protect `ingredients_vec` as well and predict what the
    /// first ingredient index will be. This allows ingredients to store their own indices.
    /// This may be worth refactoring in the future because it naturally adds more overhead to
    /// adding new kinds of ingredients.
    jar_map: Arc<Mutex<FxHashMap<TypeId, IngredientIndex>>>,

    /// Vector of ingredients.
    ///
    /// Immutable unless the mutex on `ingredients_map` is held.
    ingredients_vec: Arc<AppendOnlyVec<AdaptedIngredient<Db>>>,

    /// Conditional variable that is used to coordinate cancellation.
    /// When the main thread writes to the database, it blocks until each of the snapshots can be cancelled.
    cvar: Arc<Condvar>,

    /// A dummy varible that we use to coordinate how many outstanding database handles exist.
    /// This is set to `None` when dropping only.
    sync: Option<Arc<()>>,

    /// Mutex that is used to protect the `jars` field when waiting for snapshots to be dropped.
    noti_lock: Arc<parking_lot::Mutex<()>>,
}

// ANCHOR: default
impl<Db: Database> Default for Storage<Db> {
    fn default() -> Self {
        Self {
            shared: Shared {
                nonce: NONCE.nonce(),
                cvar: Arc::new(Default::default()),
                noti_lock: Arc::new(parking_lot::Mutex::new(())),
                jar_map: Default::default(),
                ingredients_vec: Arc::new(AppendOnlyVec::new()),
                sync: Some(Arc::new(())),
            },
            runtime: Runtime::default(),
        }
    }
}
// ANCHOR_END: default

impl<Db: Database> Storage<Db> {
    /// Adds the ingredients in `jar` to the database if not already present.
    /// If a jar of this type is already present, returns the index.
    fn add_or_lookup_adapted_jar_by_type<DbView>(
        &self,
        jar: &dyn Jar<DbView = DbView>,
    ) -> IngredientIndex
    where
        Db: DatabaseView<DbView>,
        DbView: ?Sized + Any,
    {
        let jar_type_id = jar.type_id();
        let mut jar_map = self.shared.jar_map.lock();
        *jar_map
        .entry(jar_type_id)
        .or_insert_with(|| {
            let index = IngredientIndex::from(self.shared.ingredients_vec.len());
            let ingredients = jar.create_ingredients(index);
            for ingredient in ingredients {
                let expected_index = ingredient.ingredient_index();
                let actual_index = self
                    .shared
                    .ingredients_vec
                    .push(AdaptedIngredient::new(ingredient));
                assert_eq!(
                    expected_index.as_usize(),
                    actual_index,
                    "index predicted for ingredient (`{:?}`) does not align with assigned index (`{:?}`)",
                    expected_index,
                    actual_index,
                );
            }
            index
        })
    }

    /// Return the index of the 1st ingredient from the given jar.
    pub fn lookup_jar_by_type(&self, jar_type_id: TypeId) -> Option<IngredientIndex> {
        self.shared.jar_map.lock().get(&jar_type_id).copied()
    }

    pub fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn RawIngredient {
        self.shared.ingredients_vec[index.as_usize()].unadapted_ingredient()
    }

    pub fn snapshot(&self) -> Storage<Db>
    where
        Db: ParallelDatabase,
    {
        Self {
            shared: self.shared.clone(),
            runtime: self.runtime.snapshot(),
        }
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_other_workers(&mut self) {
        loop {
            self.runtime.set_cancellation_flag();

            // Acquire lock before we check if we have unique access to the jars.
            // If we do not yet have unique access, we will go to sleep and wait for
            // the snapshots to be dropped, which will signal the cond var associated
            // with this lock.
            //
            // NB: We have to acquire the lock first to ensure that we can check for
            // unique access and go to sleep waiting on the condvar atomically,
            // as described in PR #474.
            let mut guard = self.shared.noti_lock.lock();

            // If we have unique access to the ingredients vec, we are done.
            if Arc::get_mut(self.shared.sync.as_mut().unwrap()).is_some() {
                return;
            }

            // Otherwise, wait until some other storage entities have dropped.
            //
            // The cvar `self.shared.cvar` is notified by the `Drop` impl.
            self.shared.cvar.wait(&mut guard);
        }
    }
    // ANCHOR_END: cancel_other_workers
}

impl<Db: Database> Clone for Shared<Db> {
    fn clone(&self) -> Self {
        Self {
            nonce: self.nonce.clone(),
            jar_map: self.jar_map.clone(),
            ingredients_vec: self.ingredients_vec.clone(),
            cvar: self.cvar.clone(),
            noti_lock: self.noti_lock.clone(),
            sync: self.sync.clone(),
        }
    }
}

impl<Db: Database> Drop for Storage<Db> {
    fn drop(&mut self) {
        // Careful: if this is a snapshot on the main handle,
        // we need to notify `shared.cvar` to make sure that the
        // master thread wakes up. *And*, when it does wake-up, we need to be sure
        // that the ref count on `self.shared.sync` has already been decremented.
        // So we take the value of `self.shared.sync` now and then notify the cvar.
        //
        // If this is the master thread, this dance has no real effect.
        let _guard = self.shared.noti_lock.lock();
        drop(self.shared.sync.take());
        self.shared.cvar.notify_all();
    }
}

// ANCHOR: HasJarsDyn
/// Dyn friendly subset of HasJars
pub trait HasJarsDyn: 'static {
    fn runtime(&self) -> &Runtime;

    fn runtime_mut(&mut self) -> &mut Runtime;

    fn ingredient(&self, index: IngredientIndex) -> &dyn RawIngredient;

    fn jar_index_by_type_id(&self, type_id: TypeId) -> Option<IngredientIndex>;

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

pub trait StorageForView<DbView: ?Sized> {
    fn nonce(&self) -> Nonce<StorageNonce>;

    /// Lookup the index assigned to the given jar (if any). This lookup is based purely on the jar's type.
    fn lookup_jar_by_type(&self, jar: &dyn Jar<DbView = DbView>) -> Option<IngredientIndex>;

    /// Adds a jar to the database, returning the index of the first ingredient.
    /// If a jar of this type is already present, returns the existing index.
    fn add_or_lookup_jar_by_type(&self, jar: &dyn Jar<DbView = DbView>) -> IngredientIndex;

    /// Gets an ingredient by index
    fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn RawIngredient;
}

impl<DbView, Db> StorageForView<DbView> for Storage<Db>
where
    Db: DatabaseView<DbView>,
    DbView: ?Sized + Any,
{
    fn add_or_lookup_jar_by_type(&self, jar: &dyn Jar<DbView = DbView>) -> IngredientIndex {
        self.add_or_lookup_adapted_jar_by_type(jar)
    }

    fn nonce(&self) -> Nonce<StorageNonce> {
        self.shared.nonce
    }

    fn lookup_jar_by_type(&self, jar: &dyn Jar<DbView = DbView>) -> Option<IngredientIndex> {
        self.lookup_jar_by_type(jar.type_id())
    }

    fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn RawIngredient {
        self.lookup_ingredient(index)
    }
}

/// Caches a pointer to an ingredient in a database.
/// Optimized for the case of a single database.
pub struct IngredientCache<I, DbView>
where
    I: Ingredient<DbView = DbView>,
    DbView: ?Sized,
{
    cached_data: std::sync::OnceLock<(Nonce<StorageNonce>, *const I)>,
}

impl<I, DbView> IngredientCache<I, DbView>
where
    I: Ingredient<DbView = DbView>,
    DbView: ?Sized,
{
    /// Get a reference to the ingredient in the database.
    /// If the ingredient is not already in the cache, it will be created.
    pub fn get_or_create<'s>(
        &self,
        storage: &'s dyn StorageForView<DbView>,
        create_index: impl Fn() -> IngredientIndex,
    ) -> &'s I {
        let &(nonce, ingredient) = self.cached_data.get_or_init(|| {
            let ingredient = self.create_ingredient(storage, &create_index);
            (storage.nonce(), ingredient as *const I)
        });

        if storage.nonce() == nonce {
            unsafe { &*ingredient }
        } else {
            self.create_ingredient(storage, &create_index)
        }
    }

    fn create_ingredient<'s>(
        &self,
        storage: &'s dyn StorageForView<DbView>,
        create_index: &impl Fn() -> IngredientIndex,
    ) -> &'s I {
        let index = create_index();
        storage.lookup_ingredient(index).assert_type::<I>()
    }
}
