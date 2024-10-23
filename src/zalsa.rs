use append_only_vec::AppendOnlyVec;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::thread::ThreadId;

use crate::ingredient::{Ingredient, Jar, JarAux};
use crate::nonce::{Nonce, NonceGenerator};
use crate::runtime::{Runtime, WaitResult};
use crate::table::memo::MemoTable;
use crate::table::sync::SyncTable;
use crate::table::Table;
use crate::views::Views;
use crate::zalsa_local::ZalsaLocal;
use crate::{Database, DatabaseKeyIndex, Durability, Id, Revision};

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
    /// **WARNING:** Triggers a new revision, canceling other database handles.
    /// This can lead to deadlock!
    #[doc(hidden)]
    fn zalsa_mut(&mut self) -> &mut Zalsa;

    /// Access the thread-local state associated with this database
    #[doc(hidden)]
    fn zalsa_local(&self) -> &ZalsaLocal;
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
        assert!(v < (u32::MAX as usize));
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
        assert!(u < u32::MAX as usize);
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

    /// Number of memo ingredient indices created by calls to [`next_memo_ingredient_index`](`Self::next_memo_ingredient_index`)
    memo_ingredients: Mutex<Vec<IngredientIndex>>,

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
    ingredients_vec: AppendOnlyVec<Box<dyn Ingredient>>,

    /// Indices of ingredients that require reset when a new revision starts.
    ingredients_requiring_reset: AppendOnlyVec<IngredientIndex>,

    /// The runtime for this particular salsa database handle.
    /// Each handle gets its own runtime, but the runtimes have shared state between them.
    runtime: Runtime,
}

impl Zalsa {
    pub(crate) fn new<Db: Database>() -> Self {
        Self {
            views_of: Views::new::<Db>(),
            nonce: NONCE.nonce(),
            jar_map: Default::default(),
            ingredients_vec: AppendOnlyVec::new(),
            ingredients_requiring_reset: AppendOnlyVec::new(),
            runtime: Runtime::default(),
            memo_ingredients: Default::default(),
        }
    }

    pub(crate) fn views(&self) -> &Views {
        &self.views_of
    }

    pub(crate) fn nonce(&self) -> Nonce<StorageNonce> {
        self.nonce
    }

    /// Returns the [`Table`][] used to store the value of salsa structs
    pub(crate) fn table(&self) -> &Table {
        self.runtime.table()
    }

    /// Returns the [`MemoTable`][] for the salsa struct with the given id
    pub(crate) fn memo_table_for(&self, id: Id) -> &MemoTable {
        // SAFETY: We are supply the correct current revision
        unsafe { self.table().memos(id, self.current_revision()) }
    }

    /// Returns the [`SyncTable`][] for the salsa struct with the given id
    pub(crate) fn sync_table_for(&self, id: Id) -> &SyncTable {
        // SAFETY: We are supply the correct current revision
        unsafe { self.table().syncs(id, self.current_revision()) }
    }

    /// **NOT SEMVER STABLE**
    pub fn add_or_lookup_jar_by_type(&self, jar: &dyn Jar) -> IngredientIndex {
        {
            let jar_type_id = jar.type_id();
            let mut jar_map = self.jar_map.lock();
            *jar_map
            .entry(jar_type_id)
            .or_insert_with(|| {
                let index = IngredientIndex::from(self.ingredients_vec.len());
                let ingredients = jar.create_ingredients(self, index);
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
                        self.ingredients_vec[actual_index],
                        expected_index,
                        actual_index,
                    );

                }
                index
            })
        }
    }

    pub(crate) fn lookup_ingredient(&self, index: IngredientIndex) -> &dyn Ingredient {
        &*self.ingredients_vec[index.as_usize()]
    }

    /// **NOT SEMVER STABLE**
    pub fn lookup_ingredient_mut(
        &mut self,
        index: IngredientIndex,
    ) -> (&mut dyn Ingredient, &mut Runtime) {
        (
            &mut *self.ingredients_vec[index.as_usize()],
            &mut self.runtime,
        )
    }

    /// **NOT SEMVER STABLE**
    pub fn current_revision(&self) -> Revision {
        self.runtime.current_revision()
    }

    pub(crate) fn load_cancellation_flag(&self) -> bool {
        self.runtime.load_cancellation_flag()
    }

    pub(crate) fn report_tracked_write(&mut self, durability: Durability) {
        self.runtime.report_tracked_write(durability)
    }

    /// **NOT SEMVER STABLE**
    pub fn last_changed_revision(&self, durability: Durability) -> Revision {
        self.runtime.last_changed_revision(durability)
    }

    pub(crate) fn set_cancellation_flag(&self) {
        self.runtime.set_cancellation_flag()
    }

    /// Triggers a new revision. Invoked automatically when you call `zalsa_mut`
    /// and so doesn't need to be called otherwise.
    pub(crate) fn new_revision(&mut self) -> Revision {
        let new_revision = self.runtime.new_revision();

        for index in self.ingredients_requiring_reset.iter() {
            self.ingredients_vec[index.as_usize()].reset_for_new_revision();
        }

        new_revision
    }

    /// See [`Runtime::block_on_or_unwind`][]
    pub(crate) fn block_on_or_unwind<QueryMutexGuard>(
        &self,
        db: &dyn Database,
        local_state: &ZalsaLocal,
        database_key: DatabaseKeyIndex,
        other_id: ThreadId,
        query_mutex_guard: QueryMutexGuard,
    ) {
        self.runtime
            .block_on_or_unwind(db, local_state, database_key, other_id, query_mutex_guard)
    }

    /// See [`Runtime::unblock_queries_blocked_on`][]
    pub(crate) fn unblock_queries_blocked_on(
        &self,
        database_key: DatabaseKeyIndex,
        wait_result: WaitResult,
    ) {
        self.runtime
            .unblock_queries_blocked_on(database_key, wait_result)
    }

    pub(crate) fn ingredient_index_for_memo(
        &self,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> IngredientIndex {
        self.memo_ingredients.lock()[memo_ingredient_index.as_usize()]
    }
}

impl JarAux for Zalsa {
    fn next_memo_ingredient_index(&self, ingredient_index: IngredientIndex) -> MemoIngredientIndex {
        let mut memo_ingredients = self.memo_ingredients.lock();
        let mi = MemoIngredientIndex(u32::try_from(memo_ingredients.len()).unwrap());
        memo_ingredients.push(ingredient_index);
        mi
    }
}

/// Caches a pointer to an ingredient in a database.
/// Optimized for the case of a single database.
pub struct IngredientCache<I>
where
    I: Ingredient,
{
    cached_data: std::sync::OnceLock<(Nonce<StorageNonce>, IngredientIndex)>,
    phantom: PhantomData<fn() -> I>,
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
            phantom: PhantomData,
        }
    }

    /// Get a reference to the ingredient in the database.
    /// If the ingredient is not already in the cache, it will be created.
    pub fn get_or_create<'s>(
        &self,
        db: &'s dyn Database,
        create_index: impl Fn() -> IngredientIndex,
    ) -> &'s I {
        let zalsa = db.zalsa();
        let (nonce, index) = self.cached_data.get_or_init(|| {
            let index = create_index();
            (zalsa.nonce(), index)
        });

        // FIXME: We used to cache a raw pointer to the revision but miri
        // was reporting errors because that pointer was derived from an `&`
        // that is invalidated when the next revision starts with an `&mut`.
        //
        // We could fix it with orxfun/orx-concurrent-vec#18 or by "refreshing" the cache
        // when the revision changes but just caching the index is an awful lot simpler.

        if db.zalsa().nonce() == *nonce {
            zalsa.lookup_ingredient(*index).assert_type::<I>()
        } else {
            let index = create_index();
            zalsa.lookup_ingredient(index).assert_type::<I>()
        }
    }
}

/// Given a wide pointer `T`, extracts the data pointer (typed as `U`).
///
/// # Safety requirement
///
/// `U` must be correct type for the data pointer.
pub(crate) unsafe fn transmute_data_ptr<T: ?Sized, U>(t: &T) -> &U {
    let t: *const T = t;
    let u: *const U = t as *const U;
    unsafe { &*u }
}
