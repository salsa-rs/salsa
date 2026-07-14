use std::any::TypeId;
use std::borrow::Cow;
use std::cell::UnsafeCell;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use crossbeam_utils::CachePadded;
use rustc_hash::FxBuildHasher;

use crate::function::VerifyResult;
use crate::hash::{FxHashSet, FxIndexSet};
use crate::id::{AsId, FromId};
use crate::ingredient::Ingredient;
use crate::plumbing::{self, Jar, ZalsaLocal};
use crate::sync::{Arc, Mutex, OnceLock};
use crate::table::Slot;
use crate::table::memo::{MemoTable, MemoTableTypes, MemoTableWithTypesMut};
use crate::zalsa::{IngredientIndex, JarKind, Zalsa};
use crate::zalsa_local::QueryEdge;
use crate::{DatabaseKeyIndex, Event, EventKind, Id, Revision};

mod eviction;

pub use eviction::{EvictionPolicy, Lru, LruSelector, NoopEviction, SelectLru};

#[cfg(not(test))]
const DEFAULT_REVISIONS: usize = 3;

// More aggressive garbage collection by default when testing.
#[cfg(test)]
const DEFAULT_REVISIONS: usize = 1;

/// Trait that defines the key properties of an interned struct.
///
/// Implemented by the `#[salsa::interned]` macro when applied to
/// a struct.
///
/// # Safety
///
/// For every lifetime `'db`, `Fields<'db>` must be safe for Salsa to retain
/// as `Fields<'static>` and later expose with the lifetime of a database
/// borrow. Both types must have identical layouts and validity invariants.
pub unsafe trait Configuration: Sized + 'static {
    const LOCATION: crate::ingredient::Location;
    const DEBUG_NAME: &'static str;

    /// Whether this struct should be persisted with the database.
    const PERSIST: bool;

    // The minimum number of revisions that must pass before a stale value is garbage collected.
    const REVISIONS: NonZeroUsize = NonZeroUsize::new(DEFAULT_REVISIONS).unwrap();

    /// The fields of the struct being interned.
    type Fields<'db>: InternedData;

    /// The end user struct
    type Struct<'db>: Copy + FromId + AsId;

    /// The eviction policy used by this interned ingredient.
    type Eviction: EvictionPolicy;

    /// Returns the size of any heap allocations in the output value, in bytes.
    fn heap_size(_value: &Self::Fields<'_>) -> Option<usize> {
        None
    }

    /// Serialize the fields using `serde`.
    ///
    /// Panics if the value is not persistable, i.e. `Configuration::PERSIST` is `false`.
    fn serialize<S>(value: &Self::Fields<'_>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: plumbing::serde::Serializer;

    /// Deserialize the fields using `serde`.
    ///
    /// Panics if the value is not persistable, i.e. `Configuration::PERSIST` is `false`.
    fn deserialize<'de, D>(deserializer: D) -> Result<Self::Fields<'static>, D::Error>
    where
        D: plumbing::serde::Deserializer<'de>;
}

pub trait InternedData: Sized + Eq + Hash + Clone + Sync + Send {}
impl<T: Eq + Hash + Clone + Sync + Send> InternedData for T {}

pub struct JarImpl<C: Configuration> {
    phantom: PhantomData<C>,
}

/// The interned ingredient hashes values of type `C::Fields` to produce an `Id`.
///
/// It used to store interned structs but also to store the ID fields of a tracked struct.
/// The selected eviction policy controls whether interned values are garbage collected and their
/// memory reused.
pub struct IngredientImpl<C: Configuration> {
    /// Index of this ingredient in the database (used to construct database-IDs, etc).
    ingredient_index: IngredientIndex,

    /// A hasher for the sharded ID maps.
    hasher: FxBuildHasher,

    /// A shift used to determine the shard for a given hash.
    shift: u32,

    /// Sharded data that can only be accessed through a lock.
    shards: Shards<C::Eviction>,

    /// The eviction policy and any ingredient-wide eviction state.
    eviction: C::Eviction,

    memo_table_types: Arc<MemoTableTypes>,

    _marker: PhantomData<fn() -> C>,
}

type Shards<E> = Box<[CachePadded<Mutex<IngredientShard<E>>>]>;

struct IngredientShard<E: EvictionPolicy> {
    /// Maps from data to the existing interned value for that data.
    ///
    /// The pointer is stable for the lifetime of the database. Storing it directly avoids a table
    /// lookup when hashing or comparing an existing entry.
    key_map: hashbrown::HashTable<ValueKey>,

    /// Per-shard eviction state.
    eviction: E::Shard,
}

impl<E: EvictionPolicy> Default for IngredientShard<E> {
    fn default() -> Self {
        Self {
            eviction: E::Shard::default(),
            key_map: hashbrown::HashTable::new(),
        }
    }
}

// SAFETY: `shard` is immutable. Eviction state is accessed only while holding the owning
// ingredient shard lock. `fields` is mutated only while holding that lock after stale-slot reuse
// guarantees no references remain, and is read only while holding the lock or after validation in
// the current revision. `memos` supports concurrent shared access, and is mutated only with
// exclusive access or after stale-slot reuse guarantees no shared references remain.
unsafe impl<C: Configuration> Sync for Value<C> {}

/// Struct storing the interned fields.
pub struct Value<C>
where
    C: Configuration,
{
    /// The index of the shard containing this value.
    ///
    /// This is immutable after construction and may be read to locate the protecting shard lock.
    shard: u16,

    /// Per-value eviction state.
    eviction: <C::Eviction as EvictionPolicy>::Entry,

    /// The interned fields for this value.
    ///
    /// These are valid for read-only access as long as the lock is held
    /// or the value has been validated in the current revision.
    fields: UnsafeCell<C::Fields<'static>>,

    /// Memos attached to this interned value.
    ///
    /// This is valid for read-only access as long as the lock is held
    /// or the value has been validated in the current revision.
    memos: UnsafeCell<MemoTable>,

    /// The minimum durability of all inputs consumed by the creator query.
    ///
    /// Durability determines whether the value is added to the LRU list, but the LRU scan does
    /// not read it. Keeping this byte outside `LruEntry` lets it occupy padding in the outer value;
    /// adding it to the already aligned entry would increase the entry's size.
    /// This may only be accessed while holding the value's shard lock, or with exclusive access
    /// to the database.
    durability: <C::Eviction as EvictionPolicy>::Durability,
}

impl<C> Value<C>
where
    C: Configuration,
{
    /// Fields of this interned struct.
    #[cfg(feature = "salsa_unstable")]
    pub fn fields<'db>(&'db self) -> &'db C::Fields<'db> {
        // SAFETY: The fact that this function is safe is technically unsound. However, interned
        // values are only exposed if they have been validated in the current revision, which
        // ensures that they are not reused while being accessed.
        // SAFETY: Guaranteed by `Configuration`; the restored lifetime is
        // bounded by the borrow of this interned value.
        unsafe { std::mem::transmute::<&C::Fields<'static>, &C::Fields<'db>>(&*self.fields.get()) }
    }

    /// Returns memory usage information about the interned value.
    ///
    /// # Safety
    ///
    /// The `MemoTable` must belong to a `Value` of the correct type. Additionally, the
    /// lock must be held for the shard containing the value.
    #[cfg(all(not(feature = "shuttle"), feature = "salsa_unstable"))]
    unsafe fn memory_usage(&self, memo_table_types: &MemoTableTypes) -> crate::database::SlotInfo {
        let heap_size = C::heap_size(self.fields());
        // SAFETY: The caller guarantees we hold the lock for the shard containing the value, so we
        // have at-least read-only access to the value's memos.
        let memos = unsafe { &*self.memos.get() };
        // SAFETY: The caller guarantees this is the correct types table.
        let memos = unsafe { memo_table_types.attach_memos(memos) };

        crate::database::SlotInfo {
            debug_name: C::DEBUG_NAME,
            size_of_metadata: std::mem::size_of::<Self>()
                - std::mem::size_of::<C::Fields<'static>>(),
            size_of_fields: std::mem::size_of::<C::Fields<'static>>(),
            heap_size_of_fields: heap_size,
            memos: memos.memory_usage(),
        }
    }
}

impl<C: Configuration> Default for JarImpl<C> {
    fn default() -> Self {
        Self {
            phantom: PhantomData,
        }
    }
}

impl<C: Configuration> Jar for JarImpl<C> {
    fn create_ingredients(
        _zalsa: &mut Zalsa,
        first_index: IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(IngredientImpl::<C>::new(first_index)) as _]
    }

    fn id_struct_type_id() -> TypeId {
        TypeId::of::<C::Struct<'static>>()
    }
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn new(ingredient_index: IngredientIndex) -> Self {
        let shards = new_shards::<C::Eviction>();
        let shift = usize::BITS - shards.len().trailing_zeros();

        Self {
            ingredient_index,
            hasher: FxBuildHasher,
            memo_table_types: Arc::new(MemoTableTypes::default()),
            eviction: C::Eviction::new(C::REVISIONS),
            shift,
            shards,
            _marker: PhantomData,
        }
    }
    /// Returns the shard for a given hash.
    ///
    /// Note that this value is guaranteed to be in-bounds for `self.shards`.
    #[inline]
    fn shard(&self, hash: u64) -> usize {
        // https://github.com/xacrimon/dashmap/blob/366ce7e7872866a06de66eb95002fa6cf2c117a7/src/lib.rs#L421
        ((hash as usize) << 7) >> self.shift
    }

    /// # Safety
    ///
    /// The `from_internal_data` function must be called to restore the correct lifetime
    /// before access.
    unsafe fn to_internal_data<'db>(&'db self, data: C::Fields<'db>) -> C::Fields<'static> {
        // SAFETY: Guaranteed by `Configuration` and retained in this ingredient.
        unsafe { std::mem::transmute::<C::Fields<'db>, C::Fields<'static>>(data) }
    }

    fn from_internal_data<'db>(data: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
        // SAFETY: Guaranteed by `Configuration`; the restored lifetime is
        // bounded by the borrow of the retained data.
        unsafe { std::mem::transmute::<&'db C::Fields<'static>, &'db C::Fields<'db>>(data) }
    }

    /// Intern data to a unique reference.
    ///
    /// If `key` is already interned, returns the existing [`Id`] for the interned data without
    /// invoking `assemble`.
    ///
    /// Otherwise, invokes `assemble` with the given `key` and the [`Id`] to be allocated for this
    /// interned value. The resulting [`C::Data`] will then be interned.
    ///
    /// Note: Using the database within the `assemble` function may result in a deadlock if
    /// the database ends up trying to intern or allocate a new value.
    pub fn intern<'db, Key>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        key: Key,
        assemble: impl FnOnce(Id, Key) -> C::Fields<'db>,
    ) -> C::Struct<'db>
    where
        Key: Hash,
        C::Fields<'db>: HashEqLike<Key>,
    {
        FromId::from_id(self.intern_id(zalsa, zalsa_local, key, assemble))
    }

    /// Intern data to a unique reference.
    ///
    /// If `key` is already interned, returns the existing [`Id`] for the interned data without
    /// invoking `assemble`.
    ///
    /// Otherwise, invokes `assemble` with the given `key` and the [`Id`] to be allocated for this
    /// interned value. The resulting [`C::Data`] will then be interned.
    ///
    /// Note: Using the database within the `assemble` function may result in a deadlock if
    /// the database ends up trying to intern or allocate a new value.
    pub fn intern_id<'db, Key>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        key: Key,
        assemble: impl FnOnce(Id, Key) -> C::Fields<'db>,
    ) -> crate::Id
    where
        Key: Hash,
        // We'd want the following predicate, but this currently implies `'static` due to a rustc
        // bug
        // for<'db> C::Data<'db>: HashEqLike<Key>,
        // so instead we go with this and transmute the lifetime in the `eq` closure
        C::Fields<'db>: HashEqLike<Key>,
    {
        let current_revision = zalsa.current_revision();
        self.eviction.record_revision(current_revision);

        // Hash the value before acquiring the lock.
        let hash = self.hasher.hash_one(&key);

        let shard_index = self.shard(hash);
        // SAFETY: `shard_index` is guaranteed to be in-bounds for `self.shards`.
        let shard = unsafe { &mut *self.shards.get_unchecked(shard_index).lock() };

        // SAFETY: We hold the lock for the shard containing the value.
        let eq = |value: &ValueKey| unsafe { Self::value_eq(value.value::<C>(), &key) };

        // Attempt a fast-path lookup of already interned data.
        if let Some(value) = shard.key_map.find(hash, eq) {
            // SAFETY: Values remain allocated for the lifetime of the database.
            let value = unsafe { value.value::<C>() };

            // SAFETY: We hold the lock for the shard containing the value.
            let id = unsafe { C::Eviction::id(&value.eviction) };

            // SAFETY: We hold the lock for the shard containing the value.
            return unsafe {
                self.eviction.intern_existing(eviction::InternExisting {
                    zalsa,
                    zalsa_local,
                    index: self.database_key_index(id),
                    current_revision,
                    entry: eviction::entry_ptr(value),
                    durability: &value.durability,
                    shard: &mut shard.eviction,
                })
            };
        }

        self.eviction.intern_missing(eviction::InternMissing {
            ingredient: self,
            zalsa,
            zalsa_local,
            key,
            assemble,
            key_map: &mut shard.key_map,
            shard: &mut shard.eviction,
            shard_index,
            hash,
            current_revision,
        })
    }

    /// Inserts a newly interned value into the eviction state and key map.
    ///
    /// # Safety
    ///
    /// `shard` must be a locked shard from this ingredient, and `value` must be the live, stable
    /// `Value<C>` allocated in that shard.
    unsafe fn insert_value(
        &self,
        key_map: &mut hashbrown::HashTable<ValueKey>,
        shard: &mut <C::Eviction as EvictionPolicy>::Shard,
        hash: u64,
        value: &Value<C>,
    ) {
        // SAFETY: We hold the shard lock, and the value is live in this shard.
        unsafe {
            self.eviction
                .insert_entry(shard, eviction::entry_ptr(value), &value.durability)
        };

        // SAFETY: We hold the lock for the shard containing every value passed to `hasher`.
        let hasher = |value: &ValueKey| unsafe { self.value_hash(value.value::<C>()) };
        let value_key = ValueKey::new(value);
        insert_unique_erased(key_map, hash, value_key, &hasher);

        debug_assert_eq!(hash, hasher(&value_key));
    }

    /// Clears the given memo table.
    ///
    /// # Safety
    ///
    /// `memo_table` must belong to the value identified by `id` in this ingredient. The caller
    /// must have exclusive access to the table.
    pub(crate) unsafe fn clear_memos(&self, zalsa: &Zalsa, memo_table: &mut MemoTable, id: Id) {
        /// Clears the given memo table without depending on `C`.
        ///
        /// # Safety
        ///
        /// `memo_table` must belong to the value identified by `id` in `ingredient_index` and must
        /// have been created with `memo_table_types`. The caller must have exclusive access to the
        /// table.
        unsafe fn inner(
            zalsa: &Zalsa,
            ingredient_index: IngredientIndex,
            memo_table_types: &MemoTableTypes,
            memo_table: &mut MemoTable,
            id: Id,
        ) {
            // SAFETY: The caller guarantees this is the correct types table.
            let table = unsafe { memo_table_types.attach_memos_mut(memo_table) };

            // `Database::salsa_event` is a user supplied callback which may panic
            // in that case we need a drop guard to free the memo table
            struct TableDropGuard<'a>(MemoTableWithTypesMut<'a>);

            impl Drop for TableDropGuard<'_> {
                fn drop(&mut self) {
                    // SAFETY: We have `&mut MemoTable`, so no more references to these memos exist and we are good
                    // to drop them.
                    unsafe { self.0.drop() };
                }
            }

            let mut table_guard = TableDropGuard(table);

            // SAFETY: We have `&mut MemoTable`, so no more references to these memos exist and we are good
            // to drop them.
            unsafe {
                table_guard.0.take_memos(|memo_ingredient_index, memo| {
                    let ingredient_index =
                        zalsa.ingredient_index_for_memo(ingredient_index, memo_ingredient_index);

                    let executor = DatabaseKeyIndex::new(ingredient_index, id);

                    zalsa.event(&|| Event::new(EventKind::DidDiscard { key: executor }));

                    memo.remove_outputs(zalsa, executor);
                })
            };

            std::mem::forget(table_guard);

            // Reset the table after having dropped any memos.
            memo_table.reset();
        }

        // SAFETY: The caller guarantees the table belongs to this ingredient's value, so these are
        // the correct ingredient index and memo-table types.
        unsafe {
            inner(
                zalsa,
                self.ingredient_index,
                &self.memo_table_types,
                memo_table,
                id,
            )
        }
    }

    // Hashes the value by its fields.
    //
    // # Safety
    //
    // The lock must be held for the shard containing the value.
    unsafe fn value_hash(&self, value: &Value<C>) -> u64 {
        // SAFETY: We hold the lock for the shard containing the value.
        unsafe { self.hasher.hash_one(&*value.fields.get()) }
    }

    // Compares the value by its fields to the given key.
    //
    // # Safety
    //
    // The lock must be held for the shard containing the value.
    unsafe fn value_eq<'db, Key>(value: &'db Value<C>, key: &Key) -> bool
    where
        C::Fields<'db>: HashEqLike<Key>,
    {
        // SAFETY: We hold the lock for the shard containing the value.
        let fields = unsafe { &*value.fields.get() };

        HashEqLike::eq(Self::from_internal_data(fields), key)
    }

    /// Returns the database key index for an interned value with the given id.
    #[inline]
    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.ingredient_index, id)
    }

    /// Lookup the data for an interned value based on its ID.
    pub fn data<'db>(&'db self, zalsa: &'db Zalsa, id: Id) -> &'db C::Fields<'db> {
        let value = zalsa.table().get::<Value<C>>(id);

        debug_assert!(
            !C::Eviction::CAN_REUSE || {
                let _shard = self.shards[value.shard as usize].lock();

                // SAFETY: We hold the lock for the shard containing the value.
                unsafe {
                    self.eviction
                        .is_valid(zalsa, &value.eviction, &value.durability)
                }
            },
            "Data for reusable `{database_key:?}` was not interned in the latest revision for its durability.",
            database_key = self.database_key_index(id),
        );

        // SAFETY: Reusable interned values are only exposed if they have been validated
        // in the current revision, as checked by the assertion above, which ensures that
        // they are not reused while being accessed. Non-reusable values are never reused.
        unsafe { Self::from_internal_data(&*value.fields.get()) }
    }

    /// Lookup the fields from an interned struct.
    ///
    /// Note that this is not "leaking" since no dependency edge is required.
    pub fn fields<'db>(&'db self, zalsa: &'db Zalsa, s: C::Struct<'db>) -> &'db C::Fields<'db> {
        self.data(zalsa, AsId::as_id(&s))
    }

    pub fn reset(&mut self, zalsa_mut: &mut Zalsa) {
        _ = zalsa_mut;

        for shard in self.shards.iter_mut() {
            // We can clear the key maps now that we have cancelled all other handles.
            shard.get_mut().key_map.clear();
        }
    }

    /// Returns all data corresponding to the interned struct.
    pub fn entries<'db>(&'db self, zalsa: &'db Zalsa) -> impl Iterator<Item = StructEntry<'db, C>> {
        // SAFETY: `should_lock` is `true`
        unsafe { self.entries_inner(true, zalsa) }
    }

    /// Returns all data corresponding to the interned struct.
    ///
    /// # Safety
    ///
    /// If `should_lock` is `false`, the caller *must* hold the locks for all shards
    /// of the key map.
    unsafe fn entries_inner<'db>(
        &'db self,
        should_lock: bool,
        zalsa: &'db Zalsa,
    ) -> impl Iterator<Item = StructEntry<'db, C>> {
        // TODO: Grab all locks eagerly.
        zalsa.table().slots_of::<Value<C>>().map(move |(_, value)| {
            let id = if should_lock {
                let _shard =
                    // SAFETY: `value.shard` is guaranteed to be in-bounds for `self.shards`.
                    unsafe { self.shards.get_unchecked(value.shard as usize) }.lock();

                // SAFETY: We hold the lock for the shard containing the value.
                unsafe { C::Eviction::id(&value.eviction) }
            } else {
                // SAFETY: The caller guarantees we hold the lock for the shard containing the value.
                unsafe { C::Eviction::id(&value.eviction) }
            };

            StructEntry {
                value,
                key: self.database_key_index(id),
            }
        })
    }
}

/// Creates the sharded storage once per eviction policy, rather than once per interned
/// configuration.
fn new_shards<E: EvictionPolicy>() -> Shards<E> {
    static SHARDS: OnceLock<usize> = OnceLock::new();
    let shards = *SHARDS.get_or_init(|| {
        let num_cpus = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);

        (num_cpus * 4).next_power_of_two()
    });

    (0..shards).map(|_| Default::default()).collect()
}

/// A stable pointer to an interned value allocated in the database table.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct ValueKey(NonNull<()>);

impl ValueKey {
    fn new<C: Configuration>(value: &Value<C>) -> Self {
        Self(NonNull::from(value).cast())
    }

    /// # Safety
    ///
    /// The database table containing this value must still be alive.
    unsafe fn value<'db, C: Configuration>(&self) -> &'db Value<C> {
        // SAFETY: Guaranteed by the caller. The erased pointer was created from a `Value<C>`.
        unsafe { &*self.0.as_ptr().cast::<Value<C>>() }
    }
}

// SAFETY: Values remain allocated until after the ingredient and its key map are dropped.
// Access to their mutable state is synchronized by the corresponding shard lock.
unsafe impl Send for ValueKey {}
// SAFETY: See the `Send` implementation above.
unsafe impl Sync for ValueKey {}

/// Inserts a value pointer while keeping the hasher and rehashing logic independent of `C`.
fn insert_unique_erased(
    key_map: &mut hashbrown::HashTable<ValueKey>,
    hash: u64,
    value: ValueKey,
    hasher: &dyn Fn(&ValueKey) -> u64,
) {
    key_map.insert_unique(hash, value, hasher);
}

/// An interned struct entry.
pub struct StructEntry<'db, C>
where
    C: Configuration,
{
    #[allow(dead_code)]
    value: &'db Value<C>,
    key: DatabaseKeyIndex,
}

impl<'db, C> StructEntry<'db, C>
where
    C: Configuration,
{
    /// Returns the `DatabaseKeyIndex` for this entry.
    pub fn key(&self) -> DatabaseKeyIndex {
        self.key
    }

    /// Returns the interned struct.
    pub fn as_struct(&self) -> C::Struct<'_> {
        FromId::from_id(self.key.key_index())
    }

    #[cfg(feature = "salsa_unstable")]
    pub fn value(&self) -> &'db Value<C> {
        self.value
    }
}

impl<C> Ingredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn location(&self) -> &'static crate::ingredient::Location {
        &C::LOCATION
    }

    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    unsafe fn maybe_changed_after(
        &self,
        zalsa: &crate::zalsa::Zalsa,
        _db: crate::database::RawDatabase<'_>,
        input: Id,
        _revision: Revision,
    ) -> VerifyResult {
        if !C::Eviction::CAN_REUSE {
            return VerifyResult::unchanged();
        }

        let current_revision = zalsa.current_revision();
        self.eviction.record_revision(current_revision);

        let value = zalsa.table().get::<Value<C>>(input);
        // SAFETY: `value.shard` is guaranteed to be in-bounds for `self.shards`.
        let _shard = unsafe { self.shards.get_unchecked(value.shard as usize) }.lock();

        // SAFETY: We hold the lock for the shard containing the value.
        unsafe {
            self.eviction.maybe_changed_after(
                zalsa,
                self.database_key_index(input),
                input,
                current_revision,
                &value.eviction,
            )
        }
    }

    fn collect_minimum_serialized_edges(
        &self,
        _zalsa: &Zalsa,
        edge: QueryEdge,
        serialized_edges: &mut FxIndexSet<QueryEdge>,
        _visited_edges: &mut FxHashSet<QueryEdge>,
    ) {
        if C::PERSIST && C::Eviction::CAN_REUSE {
            // If the interned struct is being persisted, it may be reachable through transitive queries.
            // Additionally, interned struct dependencies are impure in that garbage collection can
            // invalidate a dependency without a base input necessarily being updated. Thus, we must
            // preserve the transitive dependency on the interned struct, if garbage collection is
            // enabled.
            serialized_edges.insert(edge);
        }

        // Otherwise, the dependency is covered by the base inputs.
    }

    fn flatten_cycle_head_dependencies(
        &self,
        _zalsa: &Zalsa,
        id: Id,
        flattened_input_outputs: &mut FxIndexSet<QueryEdge>,
        _seen: &mut FxHashSet<DatabaseKeyIndex>,
    ) {
        flattened_input_outputs.insert(QueryEdge::input(self.database_key_index(id)));
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
    }

    fn jar_kind(&self) -> JarKind {
        JarKind::Struct
    }

    fn memo_table_types(&self) -> &Arc<MemoTableTypes> {
        &self.memo_table_types
    }

    fn memo_table_types_mut(&mut self) -> &mut Arc<MemoTableTypes> {
        &mut self.memo_table_types
    }

    /// Returns memory usage information about any interned values.
    #[cfg(all(not(feature = "shuttle"), feature = "salsa_unstable"))]
    fn memory_usage(&self, db: &dyn crate::Database) -> Option<Vec<crate::database::SlotInfo>> {
        use parking_lot::lock_api::RawMutex;

        for shard in self.shards.iter() {
            // SAFETY: We do not hold any active mutex guards.
            unsafe { shard.raw().lock() };
        }

        // SAFETY: We hold the locks for all shards.
        let entries = unsafe { self.entries_inner(false, db.zalsa()) };

        let memory_usage = entries
            // SAFETY: The memo table belongs to a value that we allocated, so it
            // has the correct type. Additionally, we are holding the locks for all shards.
            .map(|entry| unsafe { entry.value.memory_usage(&self.memo_table_types) })
            .collect();

        for shard in self.shards.iter() {
            // SAFETY: We acquired the locks for all shards.
            unsafe { shard.raw().unlock() };
        }

        Some(memory_usage)
    }

    fn is_persistable(&self) -> bool {
        C::PERSIST
    }

    fn should_serialize(&self, zalsa: &Zalsa) -> bool {
        C::PERSIST && self.entries(zalsa).next().is_some()
    }

    #[cfg(feature = "persistence")]
    unsafe fn serialize<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        f: &mut dyn FnMut(&dyn erased_serde::Serialize),
    ) {
        f(&persistence::SerializeIngredient {
            zalsa,
            ingredient: self,
        })
    }

    #[cfg(feature = "persistence")]
    fn deserialize(
        &mut self,
        zalsa: &mut Zalsa,
        deserializer: &mut dyn erased_serde::Deserializer,
    ) -> Result<(), erased_serde::Error> {
        let deserialize = persistence::DeserializeIngredient {
            zalsa,
            ingredient: self,
        };

        serde::de::DeserializeSeed::deserialize(deserialize, deserializer)
    }
}

impl<C> std::fmt::Debug for IngredientImpl<C>
where
    C: Configuration,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.ingredient_index)
            .finish()
    }
}

// SAFETY: `Value<C>` is our private type branded over the unique configuration `C`.
unsafe impl<C> Slot for Value<C>
where
    C: Configuration,
{
    #[inline(always)]
    unsafe fn memos(
        this: *const Self,
        _current_revision: Revision,
    ) -> *const crate::table::memo::MemoTable {
        // SAFETY: The fact that we have a pointer to the `Value` means it must
        // have been interned, and thus validated, in the current revision.
        // Caller obligation demands this pointer to be valid.
        unsafe { (*this).memos.get() }
    }

    #[inline(always)]
    fn memos_mut(&mut self) -> &mut crate::table::memo::MemoTable {
        self.memos.get_mut()
    }
}

/// A trait for types that hash and compare like `O`.
pub trait HashEqLike<O> {
    fn hash<H: Hasher>(&self, h: &mut H);
    fn eq(&self, data: &O) -> bool;
}

/// The `Lookup` trait is a more flexible variant on [`std::borrow::Borrow`]
/// and [`std::borrow::ToOwned`].
///
/// It is implemented by "some type that can be used as the lookup key for `O`".
/// This means that `self` can be hashed and compared for equality with values
/// of type `O` without actually creating an owned value. It `self` needs to be interned,
/// it can be converted into an equivalent value of type `O`.
///
/// The canonical example is `&str: Lookup<String>`. However, this example
/// alone can be handled by [`std::borrow::Borrow`][]. In our case, we may have
/// multiple keys accumulated into a struct, like `ViewStruct: Lookup<(K1, ...)>`,
/// where `struct ViewStruct<L1: Lookup<K1>...>(K1...)`. The `Borrow` trait
/// requires that `&(K1...)` be convertible to `&ViewStruct` which just isn't
/// possible. `Lookup` instead offers direct `hash` and `eq` methods.
pub trait Lookup<O> {
    fn into_owned(self) -> O;
}

impl<T> Lookup<T> for T {
    fn into_owned(self) -> T {
        self
    }
}

impl<T> HashEqLike<T> for T
where
    T: Hash + Eq,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h);
    }

    fn eq(&self, data: &T) -> bool {
        self == data
    }
}

impl<T> HashEqLike<T> for &T
where
    T: Hash + Eq,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(*self, &mut *h);
    }

    fn eq(&self, data: &T) -> bool {
        **self == *data
    }
}

impl<T> HashEqLike<&T> for T
where
    T: Hash + Eq,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h);
    }

    fn eq(&self, data: &&T) -> bool {
        *self == **data
    }
}

impl<T> Lookup<T> for &T
where
    T: Clone,
{
    fn into_owned(self) -> T {
        Clone::clone(self)
    }
}

impl<'a, T> HashEqLike<&'a T> for Box<T>
where
    T: ?Sized + Hash + Eq,
    Box<T>: From<&'a T>,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h)
    }
    fn eq(&self, data: &&T) -> bool {
        **self == **data
    }
}

impl<'a, T> Lookup<Box<T>> for &'a T
where
    T: ?Sized + Hash + Eq,
    Box<T>: From<&'a T>,
{
    fn into_owned(self) -> Box<T> {
        Box::from(self)
    }
}

impl<'a, T> HashEqLike<&'a T> for Arc<T>
where
    T: ?Sized + Hash + Eq,
    Arc<T>: From<&'a T>,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(&**self, &mut *h)
    }
    fn eq(&self, data: &&T) -> bool {
        **self == **data
    }
}

impl<'a, T> Lookup<Arc<T>> for &'a T
where
    T: ?Sized + Hash + Eq,
    Arc<T>: From<&'a T>,
{
    fn into_owned(self) -> Arc<T> {
        Arc::from(self)
    }
}

#[cfg(feature = "triomphe")]
impl<'a, T> HashEqLike<&'a T> for triomphe::Arc<T>
where
    T: ?Sized + Hash + Eq,
    triomphe::Arc<T>: From<&'a T>,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(&**self, &mut *h)
    }
    fn eq(&self, data: &&T) -> bool {
        **self == **data
    }
}

#[cfg(feature = "triomphe")]
impl<'a, T> Lookup<triomphe::Arc<T>> for &'a T
where
    T: ?Sized + Hash + Eq,
    triomphe::Arc<T>: From<&'a T>,
{
    fn into_owned(self) -> triomphe::Arc<T> {
        triomphe::Arc::from(self)
    }
}

impl Lookup<String> for &str {
    fn into_owned(self) -> String {
        self.to_owned()
    }
}

#[cfg(feature = "compact_str")]
impl Lookup<compact_str::CompactString> for &str {
    fn into_owned(self) -> compact_str::CompactString {
        compact_str::CompactString::new(self)
    }
}

#[cfg(feature = "compact_str")]
impl HashEqLike<&str> for compact_str::CompactString {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h)
    }

    fn eq(&self, data: &&str) -> bool {
        self == *data
    }
}

#[cfg(feature = "compact_str")]
impl HashEqLike<Cow<'_, str>> for compact_str::CompactString {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_str().hash(h);
    }

    fn eq(&self, data: &Cow<'_, str>) -> bool {
        self.as_str() == data.as_ref()
    }
}

#[cfg(feature = "compact_str")]
impl Lookup<compact_str::CompactString> for Cow<'_, str> {
    fn into_owned(self) -> compact_str::CompactString {
        compact_str::CompactString::new(Cow::into_owned(self))
    }
}

impl HashEqLike<&str> for String {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h)
    }

    fn eq(&self, data: &&str) -> bool {
        self == *data
    }
}

impl<A, T: Hash + Eq + PartialEq<A>> HashEqLike<&[A]> for Vec<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, h);
    }

    fn eq(&self, data: &&[A]) -> bool {
        self.len() == data.len() && data.iter().enumerate().all(|(i, a)| &self[i] == a)
    }
}

impl<A: Hash + Eq + PartialEq<T> + Clone + Lookup<T>, T> Lookup<Vec<T>> for &[A] {
    fn into_owned(self) -> Vec<T> {
        self.iter().map(|a| Lookup::into_owned(a.clone())).collect()
    }
}

impl<const N: usize, A, T: Hash + Eq + PartialEq<A>> HashEqLike<[A; N]> for Vec<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, h);
    }

    fn eq(&self, data: &[A; N]) -> bool {
        self.len() == data.len() && data.iter().enumerate().all(|(i, a)| &self[i] == a)
    }
}

impl<const N: usize, A: Hash + Eq + PartialEq<T> + Clone + Lookup<T>, T> Lookup<Vec<T>> for [A; N] {
    fn into_owned(self) -> Vec<T> {
        self.into_iter()
            .map(|a| Lookup::into_owned(a.clone()))
            .collect()
    }
}

impl HashEqLike<&Path> for PathBuf {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, h);
    }

    fn eq(&self, data: &&Path) -> bool {
        self == data
    }
}

impl Lookup<PathBuf> for &Path {
    fn into_owned(self) -> PathBuf {
        self.to_owned()
    }
}

impl<T: Hash + Eq + Clone> HashEqLike<Cow<'_, T>> for T {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, h);
    }

    fn eq(&self, data: &Cow<'_, T>) -> bool {
        self == data.as_ref()
    }
}

impl<T: Clone> Lookup<T> for Cow<'_, T> {
    fn into_owned(self) -> T {
        Cow::into_owned(self)
    }
}

impl HashEqLike<Cow<'_, str>> for String {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_str().hash(h);
    }

    fn eq(&self, data: &Cow<'_, str>) -> bool {
        self.as_str() == data.as_ref()
    }
}

impl Lookup<String> for Cow<'_, str> {
    fn into_owned(self) -> String {
        Cow::into_owned(self)
    }
}

impl HashEqLike<Cow<'_, Path>> for PathBuf {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_path().hash(h);
    }

    fn eq(&self, data: &Cow<'_, Path>) -> bool {
        self.as_path() == data.as_ref()
    }
}

impl Lookup<PathBuf> for Cow<'_, Path> {
    fn into_owned(self) -> PathBuf {
        Cow::into_owned(self)
    }
}

impl<T: Hash + Eq + Clone> HashEqLike<Cow<'_, [T]>> for Box<[T]> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_ref().hash(h);
    }

    fn eq(&self, data: &Cow<'_, [T]>) -> bool {
        self.as_ref() == data.as_ref()
    }
}

impl<T: Clone> Lookup<Box<[T]>> for Cow<'_, [T]> {
    fn into_owned(self) -> Box<[T]> {
        Cow::into_owned(self).into_boxed_slice()
    }
}

impl<T: Hash + Eq + Clone> HashEqLike<Cow<'_, [T]>> for Vec<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_slice().hash(h);
    }

    fn eq(&self, data: &Cow<'_, [T]>) -> bool {
        self.as_slice() == data.as_ref()
    }
}

impl<T: Clone> Lookup<Vec<T>> for Cow<'_, [T]> {
    fn into_owned(self) -> Vec<T> {
        Cow::into_owned(self)
    }
}

#[cfg(feature = "persistence")]
mod persistence {
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::hash::BuildHasher;

    use serde::ser::{SerializeMap, SerializeStruct};
    use serde::{Deserialize, de};

    use super::{Configuration, EvictionPolicy, IngredientImpl, Value};
    use crate::plumbing::Ingredient;
    use crate::table::memo::MemoTable;
    use crate::zalsa::Zalsa;
    use crate::{Durability, Id, Revision};

    pub struct SerializeIngredient<'db, C>
    where
        C: Configuration,
    {
        pub zalsa: &'db Zalsa,
        pub ingredient: &'db IngredientImpl<C>,
    }

    impl<C> serde::Serialize for SerializeIngredient<'_, C>
    where
        C: Configuration,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let Self { zalsa, ingredient } = *self;

            let count = ingredient
                .shards
                .iter()
                .map(|shard| shard.lock().key_map.len())
                .sum();

            let mut map = serializer.serialize_map(Some(count))?;

            for (_, value) in zalsa.table().slots_of::<Value<C>>() {
                // SAFETY: The safety invariant of `Ingredient::serialize` ensures we have exclusive access
                // to the database.
                let id = unsafe { C::Eviction::id(&value.eviction) };

                map.serialize_entry(&id.as_bits(), value)?;
            }

            map.end()
        }
    }

    impl<C> serde::Serialize for Value<C>
    where
        C: Configuration,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let mut value = serializer.serialize_struct("Value,", 3)?;

            let Value {
                fields,
                eviction,
                durability,
                shard: _,
                memos: _,
            } = self;

            // SAFETY: The safety invariant of `Ingredient::serialize` ensures we have exclusive access
            // to the database.
            let fields = unsafe { &*fields.get() };

            // SAFETY: The safety invariant of `Ingredient::serialize` ensures we have exclusive access
            // to the database.
            let (durability, last_interned_at) =
                unsafe { C::Eviction::serialized_metadata(eviction, durability) };

            value.serialize_field("durability", &durability)?;
            value.serialize_field("last_interned_at", &last_interned_at)?;
            value.serialize_field("fields", &SerializeFields::<C>(fields))?;

            value.end()
        }
    }

    struct SerializeFields<'db, C: Configuration>(&'db C::Fields<'static>);

    impl<C> serde::Serialize for SerializeFields<'_, C>
    where
        C: Configuration,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            C::serialize(IngredientImpl::<C>::from_internal_data(self.0), serializer)
        }
    }

    pub struct DeserializeIngredient<'db, C>
    where
        C: Configuration,
    {
        pub zalsa: &'db mut Zalsa,
        pub ingredient: &'db mut IngredientImpl<C>,
    }

    impl<'de, C> de::DeserializeSeed<'de> for DeserializeIngredient<'_, C>
    where
        C: Configuration,
    {
        type Value = ();

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_map(self)
        }
    }

    impl<'de, C> de::Visitor<'de> for DeserializeIngredient<'_, C>
    where
        C: Configuration,
    {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            let DeserializeIngredient { zalsa, ingredient } = self;

            while let Some((id, value)) = access.next_entry::<u64, DeserializeValue<C>>()? {
                let id = Id::from_bits(id);
                let (page_idx, _) = crate::table::split_id(id);

                // Determine the value shard.
                let hash = ingredient.hasher.hash_one(&value.fields.0);
                let shard_index = ingredient.shard(hash);

                // SAFETY: `shard_index` is guaranteed to be in-bounds for `self.shards`.
                let shard = unsafe { &mut *ingredient.shards.get_unchecked(shard_index).lock() };

                let value = Value::<C> {
                    shard: shard_index as u16,
                    eviction: C::Eviction::new_entry(id, value.last_interned_at),
                    fields: UnsafeCell::new(value.fields.0),
                    // SAFETY: We only ever access the memos of a value that we allocated through
                    // our `MemoTableTypes`.
                    memos: UnsafeCell::new(unsafe {
                        MemoTable::new(ingredient.memo_table_types())
                    }),
                    durability: C::Eviction::new_durability(value.durability),
                };

                // Force initialize the relevant page.
                zalsa.table_mut().force_page::<Value<C>>(
                    page_idx,
                    ingredient.ingredient_index(),
                    ingredient.memo_table_types(),
                );

                // Initialize the slot.
                //
                // SAFETY: We have a mutable reference to the database.
                let (allocated_id, value) = unsafe {
                    zalsa
                        .table()
                        .page(page_idx)
                        .allocate(page_idx, |_| value)
                        .unwrap_or_else(|_| panic!("serialized an invalid `Id`: {id:?}"))
                };

                assert_eq!(
                    allocated_id.index(),
                    id.index(),
                    "values are serialized in allocation order"
                );

                // Insert the newly allocated value into our ingredient.
                //
                // SAFETY: We hold this ingredient's shard lock, and `value` is the live, stable
                // value that we just allocated in that shard.
                unsafe {
                    ingredient.insert_value(&mut shard.key_map, &mut shard.eviction, hash, value)
                };
            }

            Ok(())
        }
    }

    #[derive(Deserialize)]
    #[serde(rename = "Value")]
    pub struct DeserializeValue<C: Configuration> {
        durability: Durability,
        last_interned_at: Revision,
        #[serde(bound = "C: Configuration")]
        fields: DeserializeFields<C>,
    }

    struct DeserializeFields<C: Configuration>(C::Fields<'static>);

    impl<'de, C> serde::Deserialize<'de> for DeserializeFields<C>
    where
        C: Configuration,
    {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            C::deserialize(deserializer)
                .map(DeserializeFields)
                .map_err(de::Error::custom)
        }
    }
}

#[cfg(all(not(feature = "shuttle"), target_pointer_width = "64"))]
mod _static_assertions {
    use std::mem;

    use super::eviction::{ImmortalLru, LruEntry, NoopEntry};
    use super::{Configuration, EvictionPolicy, Lru, NoopEviction, Value};
    use crate::{Id, plumbing};

    const _: [(); mem::size_of::<LruEntry>()] = [(); mem::size_of::<[usize; 4]>()];
    const _: [(); mem::size_of::<ImmortalLru>()] = [(); mem::size_of::<Lru>()];
    const _: [(); mem::size_of::<<ImmortalLru as EvictionPolicy>::Shard>()] =
        [(); mem::size_of::<<Lru as EvictionPolicy>::Shard>()];
    const _: [(); mem::size_of::<<ImmortalLru as EvictionPolicy>::Entry>()] =
        [(); mem::size_of::<LruEntry>()];

    const _: [(); mem::size_of::<Value<DummyConfiguration>>()] = [(); mem::size_of::<[usize; 7]>()];

    const _: [(); mem::size_of::<NoopEviction>()] = [(); 0];
    const _: [(); mem::size_of::<<NoopEviction as EvictionPolicy>::Shard>()] = [(); 0];
    const _: [(); mem::size_of::<NoopEntry>()] = [(); mem::size_of::<Id>()];
    const _: [(); mem::size_of::<Value<NoopConfiguration>>()] = [(); mem::size_of::<[usize; 4]>()];

    struct DummyConfiguration;

    // SAFETY: The fields are `[u8; 1]` for every database lifetime.
    unsafe impl Configuration for DummyConfiguration {
        const LOCATION: crate::ingredient::Location =
            crate::ingredient::Location { file: "", line: 0 };
        const DEBUG_NAME: &'static str = "";
        const PERSIST: bool = false;

        type Fields<'db> = [u8; 1];
        type Struct<'db> = Id;
        type Eviction = Lru;

        fn serialize<S>(_: &Self::Fields<'_>, _: S) -> Result<S::Ok, S::Error>
        where
            S: plumbing::serde::Serializer,
        {
            unimplemented!()
        }

        fn deserialize<'de, D>(_: D) -> Result<Self::Fields<'static>, D::Error>
        where
            D: plumbing::serde::Deserializer<'de>,
        {
            unimplemented!()
        }
    }

    struct NoopConfiguration;

    // SAFETY: The fields are `[u8; 1]` for every database lifetime.
    unsafe impl Configuration for NoopConfiguration {
        const LOCATION: crate::ingredient::Location =
            crate::ingredient::Location { file: "", line: 0 };
        const DEBUG_NAME: &'static str = "";
        const PERSIST: bool = false;

        type Fields<'db> = [u8; 1];
        type Struct<'db> = Id;
        type Eviction = NoopEviction;

        fn serialize<S>(_: &Self::Fields<'_>, _: S) -> Result<S::Ok, S::Error>
        where
            S: plumbing::serde::Serializer,
        {
            unimplemented!()
        }

        fn deserialize<'de, D>(_: D) -> Result<Self::Fields<'static>, D::Error>
        where
            D: plumbing::serde::Deserializer<'de>,
        {
            unimplemented!()
        }
    }
}
