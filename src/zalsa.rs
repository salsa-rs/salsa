use std::any::{Any, TypeId};
use std::collections::hash_map;
use std::marker::PhantomData;
use std::mem;
use std::num::NonZeroU32;
use std::panic::RefUnwindSafe;
use std::sync::atomic::Ordering;

use parking_lot::{Mutex, RwLock};
use portable_atomic::AtomicU64;
use rustc_hash::FxHashMap;

use crate::ingredient::{Ingredient, Jar};
use crate::nonce::{Nonce, NonceGenerator};
use crate::runtime::Runtime;
use crate::table::memo::{MemoEntryType, MemoTableTypes, MemoTableWithTypes};
use crate::table::sync::SyncTable;
use crate::table::Table;
use crate::views::Views;
use crate::zalsa_local::ZalsaLocal;
use crate::{Database, Durability, Id, Revision};

/// Internal plumbing trait.
///
/// [`ZalsaDatabase`] is created automatically when [`#[salsa::db]`](`crate::db`)
/// is attached to a database struct. it Contains methods that give access
/// to the internal data from the `storage` field.
///
/// # Safety
///
/// The system assumes this is implemented by a salsa procedural macro
/// which makes use of private data from the [`Storage`](`crate::storage::Storage`) struct.
/// Do not implement this yourself, instead, apply the [`#[salsa::db]`](`crate::db`) macro
/// to your database.
pub unsafe trait ZalsaDatabase: Any {
    /// Plumbing method: access both zalsa and zalsa-local at once.
    /// More efficient if you need both as it does only a single vtable dispatch.
    #[doc(hidden)]
    fn zalsas(&self) -> (&Zalsa, &ZalsaLocal) {
        (self.zalsa(), self.zalsa_local())
    }

    /// Plumbing method: Access the internal salsa methods.
    #[doc(hidden)]
    fn zalsa(&self) -> &Zalsa;

    /// Plumbing method: Access the internal salsa methods for mutating the database.
    ///
    /// **WARNING:** Triggers cancellation to other database handles.
    /// This can lead to deadlock!
    #[doc(hidden)]
    fn zalsa_mut(&mut self) -> &mut Zalsa;

    /// Access the thread-local state associated with this database
    #[doc(hidden)]
    fn zalsa_local(&self) -> &ZalsaLocal;

    /// Clone the database.
    #[doc(hidden)]
    fn fork_db(&self) -> Box<dyn Database>;
}

pub fn views<Db: ?Sized + Database>(db: &Db) -> &Views {
    db.zalsa().views()
}

/// Nonce type representing the underlying database storage.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorageNonce;

/// Generator for storage nonces.
static NONCE: NonceGenerator<StorageNonce> = NonceGenerator::new();

/// An ingredient index identifies a particular [`Ingredient`] in the database.
///
/// The database contains a number of jars, and each jar contains a number of ingredients.
/// Each ingredient is given a unique index as the database is being created.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IngredientIndex(u32);

impl IngredientIndex {
    /// Create an ingredient index from a usize.
    pub(crate) fn from(v: usize) -> Self {
        assert!(v <= u32::MAX as usize);
        Self(v as u32)
    }

    /// Convert the ingredient index back into a usize.
    pub(crate) fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub fn successor(self, index: usize) -> Self {
        IngredientIndex(self.0 + 1 + index as u32)
    }
}

/// A special secondary index *just* for ingredients that attach
/// "memos" to salsa structs (currently: just tracked functions).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MemoIngredientIndex(u32);

impl MemoIngredientIndex {
    pub(crate) fn from_usize(u: usize) -> Self {
        assert!(u <= u32::MAX as usize);
        MemoIngredientIndex(u as u32)
    }

    pub(crate) fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// The "plumbing interface" to the Salsa database. Stores all the ingredients and other data.
///
/// **NOT SEMVER STABLE.**
pub struct Zalsa {
    views_of: Views,

    nonce: Nonce<StorageNonce>,

    /// Map from the [`IngredientIndex::as_usize`][] of a salsa struct to a list of
    /// [ingredient-indices](`IngredientIndex`) for tracked functions that have this salsa struct
    /// as input.
    memo_ingredient_indices: RwLock<Vec<Vec<IngredientIndex>>>,

    /// Map from the type-id of an `impl Jar` to the index of its first ingredient.
    /// This is using a `Mutex<FxHashMap>` (versus, say, a `FxDashMap`)
    /// so that we can protect `ingredients_vec` as well and predict what the
    /// first ingredient index will be. This allows ingredients to store their own indices.
    /// This may be worth refactoring in the future because it naturally adds more overhead to
    /// adding new kinds of ingredients.
    jar_map: Mutex<FxHashMap<TypeId, IngredientIndex>>,

    /// A map from the `IngredientIndex` to the `TypeId` of its ID struct.
    ///
    /// Notably this is not the reverse mapping of `jar_map`.
    ingredient_to_id_struct_type_id_map: RwLock<FxHashMap<IngredientIndex, TypeId>>,

    /// Vector of ingredients.
    ///
    /// Immutable unless the mutex on `ingredients_map` is held.
    ingredients_vec: boxcar::Vec<Box<dyn Ingredient>>,

    /// Indices of ingredients that require reset when a new revision starts.
    ingredients_requiring_reset: boxcar::Vec<IngredientIndex>,

    /// The runtime for this particular salsa database handle.
    /// Each handle gets its own runtime, but the runtimes have shared state between them.
    runtime: Runtime,
}

/// All fields on Zalsa are locked behind [`Mutex`]es and [`RwLock`]s and cannot enter
/// inconsistent states. The contents of said fields are largely ID mappings, with the exception
/// of [`Runtime::dependency_graph`]. However, [`Runtime::dependency_graph`] does not
/// invoke any queries and as such there will be no panic from code downstream of Salsa. It can only
/// panic if an assertion inside of Salsa fails.
impl RefUnwindSafe for Zalsa {}

impl Zalsa {
    pub(crate) fn new<Db: Database>() -> Self {
        Self {
            views_of: Views::new::<Db>(),
            nonce: NONCE.nonce(),
            jar_map: Default::default(),
            ingredient_to_id_struct_type_id_map: Default::default(),
            ingredients_vec: boxcar::Vec::new(),
            ingredients_requiring_reset: boxcar::Vec::new(),
            runtime: Runtime::default(),
            memo_ingredient_indices: Default::default(),
        }
    }

    pub(crate) fn nonce(&self) -> Nonce<StorageNonce> {
        self.nonce
    }

    pub(crate) fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub(crate) fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self.runtime
    }

    /// Returns the [`Table`] used to store the value of salsa structs
    pub(crate) fn table(&self) -> &Table {
        self.runtime.table()
    }

    /// Returns the [`MemoTable`][] for the salsa struct with the given id
    pub(crate) fn memo_table_for(&self, id: Id) -> MemoTableWithTypes<'_> {
        let table = self.table();
        // SAFETY: We are supplying the correct current revision
        unsafe { table.memos(id, self.current_revision()) }
    }

    /// Returns the [`SyncTable`][] for the salsa struct with the given id
    pub(crate) fn sync_table_for(&self, id: Id) -> &SyncTable {
        // SAFETY: We are supplying the correct current revision
        unsafe { self.table().syncs(id, self.current_revision()) }
    }

    pub(crate) fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn Ingredient {
        let index = index.as_usize();
        self.ingredients_vec
            .get(index)
            .unwrap_or_else(|| panic!("index `{index}` is uninitialized"))
            .as_ref()
    }

    pub(crate) fn ingredient_index_for_memo(
        &self,
        struct_ingredient_index: IngredientIndex,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> IngredientIndex {
        self.memo_ingredient_indices.read()[struct_ingredient_index.as_usize()]
            [memo_ingredient_index.as_usize()]
    }

    /// Starts unwinding the stack if the current revision is cancelled.
    ///
    /// This method can be called by query implementations that perform
    /// potentially expensive computations, in order to speed up propagation of
    /// cancellation.
    ///
    /// Cancellation will automatically be triggered by salsa on any query
    /// invocation.
    #[inline]
    pub(crate) fn unwind_if_revision_cancelled(&self, db: &(impl Database + ?Sized)) {
        db.salsa_event(&|| crate::Event::new(crate::EventKind::WillCheckCancellation));
        if self.runtime().load_cancellation_flag() {
            db.zalsa_local().unwind_cancelled(self.current_revision());
        }
    }

    /// # Safety
    ///
    /// The memo types must be correct.
    pub(crate) unsafe fn next_memo_ingredient_index(
        &self,
        struct_ingredient_index: IngredientIndex,
        ingredient_index: IngredientIndex,
        memo_type: MemoEntryType,
        memo_types: &MemoTableTypes,
    ) -> MemoIngredientIndex {
        let mut memo_ingredients = self.memo_ingredient_indices.write();
        let idx = struct_ingredient_index.as_usize();
        let memo_ingredients = if let Some(memo_ingredients) = memo_ingredients.get_mut(idx) {
            memo_ingredients
        } else {
            memo_ingredients.resize_with(idx + 1, Vec::new);
            memo_ingredients.get_mut(idx).unwrap()
        };
        let mi = MemoIngredientIndex(u32::try_from(memo_ingredients.len()).unwrap());
        memo_ingredients.push(ingredient_index);

        memo_types.set(mi, memo_type);

        mi
    }
}

/// Semver unstable APIs used by the macro expansions
impl Zalsa {
    /// **NOT SEMVER STABLE**
    pub fn views(&self) -> &Views {
        &self.views_of
    }

    /// **NOT SEMVER STABLE**
    #[inline]
    pub fn lookup_page_type_id(&self, id: Id) -> TypeId {
        let ingredient_index = self.ingredient_index(id);
        *self
            .ingredient_to_id_struct_type_id_map
            .read()
            .get(&ingredient_index)
            .expect("should have the ingredient index available")
    }

    /// **NOT SEMVER STABLE**
    #[doc(hidden)]
    pub fn add_or_lookup_jar_by_type<J: Jar>(&self) -> IngredientIndex {
        let jar_type_id = TypeId::of::<J>();
        let mut jar_map = self.jar_map.lock();
        if let Some(index) = jar_map.get(&jar_type_id) {
            return *index;
        };
        // Drop the map as `J::create_dependencies` may recurse into this function taking the lock again.
        drop(jar_map);
        let dependencies = J::create_dependencies(self);

        jar_map = self.jar_map.lock();
        let index = IngredientIndex::from(self.ingredients_vec.count());
        match jar_map.entry(jar_type_id) {
            hash_map::Entry::Occupied(entry) => {
                // Someone made it earlier than us.
                return *entry.get();
            }
            hash_map::Entry::Vacant(entry) => entry.insert(index),
        };
        let ingredients = J::create_ingredients(self, index, dependencies);
        for ingredient in ingredients {
            let expected_index = ingredient.ingredient_index();

            if ingredient.requires_reset_for_new_revision() {
                self.ingredients_requiring_reset.push(expected_index);
            }

            let actual_index = self.ingredients_vec.push(ingredient);
            assert_eq!(
                expected_index.as_usize(),
                actual_index,
                "ingredient `{:?}` was predicted to have index `{:?}` but actually has index `{:?}`",
                self.ingredients_vec[actual_index],
                expected_index,
                actual_index,
            );
        }

        drop(jar_map);
        self.ingredient_to_id_struct_type_id_map
            .write()
            .insert(index, J::id_struct_type_id());

        index
    }

    /// **NOT SEMVER STABLE**
    #[doc(hidden)]
    pub fn lookup_ingredient_mut(
        &mut self,
        index: IngredientIndex,
    ) -> (&mut dyn Ingredient, &mut Runtime) {
        let index = index.as_usize();
        let ingredient = self
            .ingredients_vec
            .get_mut(index)
            .unwrap_or_else(|| panic!("index `{index}` is uninitialized"));
        (ingredient.as_mut(), &mut self.runtime)
    }

    /// **NOT SEMVER STABLE**
    #[doc(hidden)]
    pub fn current_revision(&self) -> Revision {
        self.runtime.current_revision()
    }

    /// **NOT SEMVER STABLE**
    #[doc(hidden)]
    pub fn last_changed_revision(&self, durability: Durability) -> Revision {
        self.runtime.last_changed_revision(durability)
    }

    /// **NOT SEMVER STABLE**
    /// Triggers a new revision.
    #[doc(hidden)]
    pub fn new_revision(&mut self) -> Revision {
        let new_revision = self.runtime.new_revision();

        for (_, index) in self.ingredients_requiring_reset.iter() {
            let index = index.as_usize();
            let ingredient = self
                .ingredients_vec
                .get_mut(index)
                .unwrap_or_else(|| panic!("index `{index}` is uninitialized"));

            ingredient.reset_for_new_revision(self.runtime.table_mut());
        }

        new_revision
    }

    /// **NOT SEMVER STABLE**
    #[doc(hidden)]
    pub fn evict_lru(&mut self) {
        for (_, index) in self.ingredients_requiring_reset.iter() {
            let index = index.as_usize();
            self.ingredients_vec
                .get_mut(index)
                .unwrap_or_else(|| panic!("index `{index}` is uninitialized"))
                .reset_for_new_revision(self.runtime.table_mut());
        }
    }

    #[inline]
    pub fn ingredient_index(&self, id: Id) -> IngredientIndex {
        self.table().ingredient_index(id)
    }
}

/// Caches a pointer to an ingredient in a database.
/// Optimized for the case of a single database.
pub struct IngredientCache<I>
where
    I: Ingredient,
{
    // A packed representation of `Option<(Nonce<StorageNonce>, IngredientIndex)>`.
    //
    // This allows us to replace a lock in favor of an atomic load. This works thanks to `Nonce`
    // having a niche, which means the entire type can fit into an `AtomicU64`.
    cached_data: AtomicU64,
    phantom: PhantomData<fn() -> I>,
}

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
    const UNINITIALIZED: u64 = 0;

    /// Create a new cache
    pub const fn new() -> Self {
        Self {
            cached_data: AtomicU64::new(Self::UNINITIALIZED),
            phantom: PhantomData,
        }
    }

    /// Get a reference to the ingredient in the database.
    /// If the ingredient is not already in the cache, it will be created.
    #[inline(never)]
    pub fn get_or_create<'s>(
        &self,
        db: &'s dyn Database,
        create_index: impl Fn() -> IngredientIndex,
    ) -> &'s I {
        const _: () = assert!(
            mem::size_of::<(Nonce<StorageNonce>, IngredientIndex)>() == mem::size_of::<u64>()
        );
        let cached_data = self.cached_data.load(Ordering::Acquire);
        if cached_data == Self::UNINITIALIZED {
            let index = create_index();
            let nonce = db.zalsa().nonce().into_u32().get() as u64;
            let packed = (nonce << u32::BITS) | (index.0 as u64);
            debug_assert_ne!(packed, Self::UNINITIALIZED);

            // Discard the result, whether we won over the cache or not does not matter
            // we know that something has been cached now
            _ = self.cached_data.compare_exchange(
                Self::UNINITIALIZED,
                packed,
                Ordering::Release,
                Ordering::Acquire,
            );
            // and we already have our index computed so we can just use that
            return db.zalsa().lookup_ingredient(index).assert_type::<I>();
        };

        // unpack our u64
        // SAFETY: We've checked against `UNINITIALIZED` (0) above and so the upper bits must be non-zero
        let nonce = Nonce::<StorageNonce>::from_u32(unsafe {
            NonZeroU32::new_unchecked((cached_data >> u32::BITS) as u32)
        });
        let mut index = IngredientIndex(cached_data as u32);

        if db.zalsa().nonce() != nonce {
            index = create_index();
        }
        db.zalsa().lookup_ingredient(index).assert_type::<I>()
    }
}

/// Given a wide pointer `T`, extracts the data pointer (typed as `U`).
///
/// # Safety
///
/// `U` must be correct type for the data pointer.
pub unsafe fn transmute_data_ptr<T: ?Sized, U>(t: &T) -> &U {
    let t: *const T = t;
    let u: *const U = t as *const U;
    // SAFETY: the caller must guarantee that `T` is a wide pointer for `U`
    unsafe { &*u }
}

/// Given a wide pointer `T`, extracts the data pointer (typed as `U`).
///
/// # Safety
///
/// `U` must be correct type for the data pointer.
pub(crate) unsafe fn transmute_data_mut_ptr<T: ?Sized, U>(t: &mut T) -> &mut U {
    let t: *mut T = t;
    let u: *mut U = t as *mut U;
    // SAFETY: the caller must guarantee that `T` is a wide pointer for `U`
    unsafe { &mut *u }
}
