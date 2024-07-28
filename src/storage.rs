use std::any::TypeId;

use orx_concurrent_vec::ConcurrentVec;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::cycle::CycleRecoveryStrategy;
use crate::database::UserData;
use crate::ingredient::{Ingredient, Jar};
use crate::nonce::{Nonce, NonceGenerator};
use crate::runtime::Runtime;
use crate::views::Views;
use crate::{Database, DatabaseImpl, Durability, Revision};

pub fn views<Db: ?Sized + Database>(db: &Db) -> &Views {
    db.zalsa().views()
}

/// The "plumbing interface" to the Salsa database.
///
/// **NOT SEMVER STABLE.**
pub trait Zalsa {
    /// Returns a reference to the underlying.
    fn views(&self) -> &Views;

    /// Returns the nonce for the underyling storage.
    ///
    /// # Safety
    ///
    /// This nonce is guaranteed to be unique for the database and never to be reused.
    fn nonce(&self) -> Nonce<StorageNonce>;

    /// Lookup the index assigned to the given jar (if any). This lookup is based purely on the jar's type.
    fn lookup_jar_by_type(&self, jar: &dyn Jar) -> Option<IngredientIndex>;

    /// Adds a jar to the database, returning the index of the first ingredient.
    /// If a jar of this type is already present, returns the existing index.
    fn add_or_lookup_jar_by_type(&self, jar: &dyn Jar) -> IngredientIndex;

    /// Gets an `&`-ref to an ingredient by index
    fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn Ingredient;

    /// Gets an `&mut`-ref to an ingredient by index.
    fn lookup_ingredient_mut(
        &mut self,
        index: IngredientIndex,
    ) -> &mut dyn Ingredient;

    fn runtimex(&self) -> &Runtime;

    /// Return the current revision
    fn current_revision(&self) -> Revision;

    /// Return the time when an input of durability `durability` last changed
    fn last_changed_revision(&self, durability: Durability) -> Revision;

    /// True if any threads have signalled for cancellation
    fn load_cancellation_flag(&self) -> bool;

    /// Signal for cancellation, indicating current thread is trying to get unique access.
    fn set_cancellation_flag(&self);

    /// Reports a (synthetic) tracked write to "some input of the given durability".
    fn report_tracked_write(&mut self, durability: Durability);
}

impl<U: UserData> Zalsa for ZalsaImpl<U> {
    fn views(&self) -> &Views {
        &self.views_of
    }

    fn nonce(&self) -> Nonce<StorageNonce> {
        self.nonce
    }

    fn lookup_jar_by_type(&self, jar: &dyn Jar) -> Option<IngredientIndex> {
        self.jar_map.lock().get(&jar.type_id()).copied()
    }

    fn add_or_lookup_jar_by_type(&self, jar: &dyn Jar) -> IngredientIndex {
        {
            let jar_type_id = jar.type_id();
            let mut jar_map = self.jar_map.lock();
            *jar_map
            .entry(jar_type_id)
            .or_insert_with(|| {
                let index = IngredientIndex::from(self.ingredients_vec.len());
                let ingredients = jar.create_ingredients(index);
                for ingredient in ingredients {
                    let expected_index = ingredient.ingredient_index();
    
                    if ingredient.requires_reset_for_new_revision() {
                        self.ingredients_requiring_reset.push(expected_index);
                    }
    
                    let actual_index = self
                        .ingredients_vec
                        .push(ingredient);
                    assert_eq!(
                        expected_index.as_usize(),
                        actual_index,
                        "ingredient `{:?}` was predicted to have index `{:?}` but actually has index `{:?}`",
                        self.ingredients_vec.get(actual_index).unwrap(),
                        expected_index,
                        actual_index,
                    );
    
                }
                index
            })
        }
    }

    fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn Ingredient {
        &**self.ingredients_vec.get(index.as_usize()).unwrap()
    }

    fn lookup_ingredient_mut(
        &mut self,
        index: IngredientIndex,
    ) -> &mut dyn Ingredient {
        &mut **self.ingredients_vec.get_mut(index.as_usize()).unwrap()
    }

    fn current_revision(&self) -> Revision {
        self.runtime.current_revision()
    }
    
    fn load_cancellation_flag(&self) -> bool {
        self.runtime.load_cancellation_flag()
    }
    
    fn report_tracked_write(&mut self, durability: Durability) {
        self.runtime.report_tracked_write(durability)
    }
    
    fn runtimex(&self) -> &Runtime {
        &self.runtime
    }
    
    fn last_changed_revision(&self, durability: Durability) -> Revision {
        self.runtime.last_changed_revision(durability)
    }
    
    fn set_cancellation_flag(&self) {
        self.runtime.set_cancellation_flag()
    }
}

/// Nonce type representing the underlying database storage.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorageNonce;

/// Generator for storage nonces.
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

    /// Convert the ingredient index back into a usize.
    pub(crate) fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn cycle_recovery_strategy(self, db: &dyn Database) -> CycleRecoveryStrategy {
        db.zalsa().lookup_ingredient(self).cycle_recovery_strategy()
    }

    pub fn successor(self, index: usize) -> Self {
        IngredientIndex(self.0 + 1 + index as u32)
    }

    /// Return the "debug name" of this ingredient (e.g., the name of the tracked struct it represents)
    pub(crate) fn debug_name(self, db: &dyn Database) -> &'static str {
        db.zalsa().lookup_ingredient(self).debug_name()
    }
}

/// The "storage" struct stores all the data for the jars.
/// It is shared between the main database and any active snapshots.
pub(crate) struct ZalsaImpl<U: UserData> {
    user_data: U,
    
    views_of: Views,

    nonce: Nonce<StorageNonce>,

    /// Map from the type-id of an `impl Jar` to the index of its first ingredient.
    /// This is using a `Mutex<FxHashMap>` (versus, say, a `FxDashMap`)
    /// so that we can protect `ingredients_vec` as well and predict what the
    /// first ingredient index will be. This allows ingredients to store their own indices.
    /// This may be worth refactoring in the future because it naturally adds more overhead to
    /// adding new kinds of ingredients.
    jar_map: Mutex<FxHashMap<TypeId, IngredientIndex>>,

    /// Vector of ingredients.
    ///
    /// Immutable unless the mutex on `ingredients_map` is held.
    ingredients_vec: ConcurrentVec<Box<dyn Ingredient>>,

    /// Indices of ingredients that require reset when a new revision starts.
    ingredients_requiring_reset: ConcurrentVec<IngredientIndex>,

    /// The runtime for this particular salsa database handle.
    /// Each handle gets its own runtime, but the runtimes have shared state between them.
    runtime: Runtime,
}

// ANCHOR: default
impl<U: UserData + Default> Default for ZalsaImpl<U> {
    fn default() -> Self {
        Self::with(Default::default())
    }
}
// ANCHOR_END: default

impl<U: UserData> ZalsaImpl<U> {
    pub(crate) fn with(user_data: U) -> Self {
        Self {
            views_of: Views::new::<DatabaseImpl<U>>(),
            nonce: NONCE.nonce(),
            jar_map: Default::default(),
            ingredients_vec: Default::default(),
            ingredients_requiring_reset: Default::default(),
            runtime: Runtime::default(),
            user_data,
        }
    }

    pub(crate) fn user_data(&self) -> &U {
        &self.user_data
    }

    /// Triggers a new revision. Invoked automatically when you call `zalsa_mut`
    /// and so doesn't need to be called otherwise.
    pub(crate) fn new_revision(&mut self) -> Revision {
        let new_revision = self.runtime.new_revision();

        for index in self.ingredients_requiring_reset.iter() {
            self.ingredients_vec
                .get_mut(index.as_usize())
                .unwrap()
                .reset_for_new_revision();
        }

        new_revision
    }
}

/// Caches a pointer to an ingredient in a database.
/// Optimized for the case of a single database.
pub struct IngredientCache<I>
where
    I: Ingredient,
{
    cached_data: std::sync::OnceLock<(Nonce<StorageNonce>, *const I)>,
}

unsafe impl<I> Sync for IngredientCache<I> where I: Ingredient + Sync {}

impl<I> Default for IngredientCache<I>
where
    I: Ingredient,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<I> IngredientCache<I>
where
    I: Ingredient,
{
    /// Create a new cache
    pub const fn new() -> Self {
        Self {
            cached_data: std::sync::OnceLock::new(),
        }
    }

    /// Get a reference to the ingredient in the database.
    /// If the ingredient is not already in the cache, it will be created.
    pub fn get_or_create<'s>(
        &self,
        db: &'s dyn Database,
        create_index: impl Fn() -> IngredientIndex,
    ) -> &'s I {
        let &(nonce, ingredient) = self.cached_data.get_or_init(|| {
            let ingredient = self.create_ingredient(db, &create_index);
            (db.zalsa().nonce(), ingredient as *const I)
        });

        if db.zalsa().nonce() == nonce {
            unsafe { &*ingredient }
        } else {
            self.create_ingredient(db, &create_index)
        }
    }

    fn create_ingredient<'s>(
        &self,
        storage: &'s dyn Database,
        create_index: &impl Fn() -> IngredientIndex,
    ) -> &'s I {
        let index = create_index();
        storage.zalsa().lookup_ingredient(index).assert_type::<I>()
    }
}
