use std::any::TypeId;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use crossbeam_utils::CachePadded;
use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink, UnsafeRef};
use rustc_hash::FxBuildHasher;

use crate::durability::Durability;
use crate::function::{VerifyCycleHeads, VerifyResult};
use crate::hash::{FxHashSet, FxIndexSet};
use crate::id::{AsId, FromId};
use crate::ingredient::Ingredient;
use crate::plumbing::{self, Jar, ZalsaLocal};
use crate::revision::AtomicRevision;
use crate::sync::{Arc, Mutex, OnceLock};
use crate::table::memo::{MemoTable, MemoTableTypes, MemoTableWithTypesMut};
use crate::table::Slot;
use crate::zalsa::{IngredientIndex, JarKind, Zalsa};
use crate::zalsa_local::QueryEdge;
use crate::{DatabaseKeyIndex, Event, EventKind, Id, Revision};

/// Trait that defines the key properties of an interned struct.
///
/// Implemented by the `#[salsa::interned]` macro when applied to
/// a struct.
pub trait Configuration: Sized + 'static {
    const LOCATION: crate::ingredient::Location;
    const DEBUG_NAME: &'static str;

    /// Whether this struct should be persisted with the database.
    const PERSIST: bool;

    // The minimum number of revisions that must pass before a stale value is garbage collected.
    #[cfg(test)]
    const REVISIONS: NonZeroUsize = NonZeroUsize::new(3).unwrap();

    #[cfg(not(test))] // More aggressive garbage collection by default when testing.
    const REVISIONS: NonZeroUsize = NonZeroUsize::new(1).unwrap();

    /// The fields of the struct being interned.
    type Fields<'db>: InternedData;

    /// The end user struct
    type Struct<'db>: Copy + FromId + AsId;

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
/// Interned values are garbage collected and their memory reused based on an LRU heuristic.
pub struct IngredientImpl<C: Configuration> {
    /// Index of this ingredient in the database (used to construct database-IDs, etc).
    ingredient_index: IngredientIndex,

    /// A hasher for the sharded ID maps.
    hasher: FxBuildHasher,

    /// A shift used to determine the shard for a given hash.
    shift: u32,

    /// Sharded data that can only be accessed through a lock.
    shards: Box<[CachePadded<Mutex<IngredientShard<C>>>]>,

    /// A queue of recent revisions in which values were interned.
    revision_queue: RevisionQueue<C>,

    memo_table_types: Arc<MemoTableTypes>,

    _marker: PhantomData<fn() -> C>,
}

struct IngredientShard<C: Configuration> {
    /// Maps from data to the existing interned ID for that data.
    ///
    /// This doesn't hold the fields themselves to save memory, instead it points
    /// to the slot ID.
    key_map: hashbrown::HashTable<Id>,

    /// An intrusive linked list for LRU.
    lru: LinkedList<ValueAdapter<C>>,
}

impl<C: Configuration> Default for IngredientShard<C> {
    fn default() -> Self {
        Self {
            lru: LinkedList::default(),
            key_map: hashbrown::HashTable::new(),
        }
    }
}

// SAFETY: `LinkedListLink` is `!Sync`, however, the linked list is only accessed through the
// ingredient lock, and values are only ever linked to a single list on the ingredient.
unsafe impl<C: Configuration> Sync for Value<C> {}

intrusive_adapter!(ValueAdapter<C> = UnsafeRef<Value<C>>: Value<C> { link: LinkedListLink } where C: Configuration);

/// Struct storing the interned fields.
pub struct Value<C>
where
    C: Configuration,
{
    /// The index of the shard containing this value.
    shard: u16,

    /// An intrusive linked list for LRU.
    link: LinkedListLink,

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

    /// Data that can only be accessed while holding the lock for the
    /// `key_map` shard containing the value ID.
    shared: UnsafeCell<ValueShared>,
}

/// Shared value data can only be read through the lock.
#[repr(Rust, packed)] // Allow `durability` to be stored in the padding of the outer `Value` struct.
#[derive(Clone, Copy)]
struct ValueShared {
    /// The interned ID for this value.
    ///
    /// Storing this on the value itself is necessary to identify slots
    /// from the LRU list, as well as keep track of the generation.
    ///
    /// Values that are reused increment the ID generation, as if they had
    /// allocated a new slot. This eliminates the need for dependency edges
    /// on queries that *read* from an interned value, as any memos dependent
    /// on the previous value will not match the new ID.
    ///
    /// However, reusing a slot invalidates the previous ID, so dependency edges
    /// on queries that *create* an interned value are still required to ensure
    /// the value is re-interned with a new ID.
    id: Id,

    /// The revision the value was most-recently interned in.
    last_interned_at: Revision,

    /// The minimum durability of all inputs consumed by the creator
    /// query prior to creating this interned struct. If any of those
    /// inputs changes, then the creator query may create this struct
    /// with different values.
    durability: Durability,
}

impl ValueShared {
    /// Returns `true` if this value slot can be reused when interning, and should be added to the LRU.
    fn is_reusable<C: Configuration>(&self) -> bool {
        // Garbage collection is disabled.
        if C::REVISIONS == IMMORTAL {
            return false;
        }

        // Collecting higher durability values requires invalidating the revision for their
        // durability (see `Database::synthetic_write`, which requires a mutable reference to
        // the database) to avoid short-circuiting calls to `maybe_changed_after`. This is
        // necessary because `maybe_changed_after` for interned values is not "pure"; it updates
        // the `last_interned_at` field before validating a given value to ensure that it is not
        // reused after read in the current revision.
        self.durability == Durability::LOW
    }
}

impl<C> Value<C>
where
    C: Configuration,
{
    /// Fields of this interned struct.
    #[cfg(feature = "salsa_unstable")]
    pub fn fields(&self) -> &C::Fields<'static> {
        // SAFETY: The fact that this function is safe is technically unsound. However, interned
        // values are only exposed if they have been validated in the current revision, which
        // ensures that they are not reused while being accessed.
        unsafe { &*self.fields.get() }
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
            size_of_metadata: std::mem::size_of::<Self>() - std::mem::size_of::<C::Fields<'_>>(),
            size_of_fields: std::mem::size_of::<C::Fields<'_>>(),
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
        static SHARDS: OnceLock<usize> = OnceLock::new();
        let shards = *SHARDS.get_or_init(|| {
            let num_cpus = std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1);

            (num_cpus * 4).next_power_of_two()
        });

        Self {
            ingredient_index,
            hasher: FxBuildHasher,
            memo_table_types: Arc::new(MemoTableTypes::default()),
            revision_queue: RevisionQueue::default(),
            shift: usize::BITS - shards.trailing_zeros(),
            shards: (0..shards).map(|_| Default::default()).collect(),
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
        // SAFETY: Guaranteed by caller.
        unsafe { std::mem::transmute(data) }
    }

    fn from_internal_data<'db>(data: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
        // SAFETY: It's sound to go from `Data<'static>` to `Data<'db>`. We shrink the
        // lifetime here to use a single lifetime in `Lookup::eq(&StructKey<'db>, &C::Data<'db>)`
        unsafe { std::mem::transmute(data) }
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
        // Record the current revision as active.
        let current_revision = zalsa.current_revision();
        self.revision_queue.record(current_revision);

        // Hash the value before acquiring the lock.
        let hash = self.hasher.hash_one(&key);

        let shard_index = self.shard(hash);
        // SAFETY: `shard_index` is guaranteed to be in-bounds for `self.shards`.
        let shard = unsafe { &mut *self.shards.get_unchecked(shard_index).lock() };

        let found_value = Cell::new(None);
        // SAFETY: We hold the lock for the shard containing the value.
        let eq = |id: &_| unsafe { Self::value_eq(*id, &key, zalsa, &found_value) };

        // Attempt a fast-path lookup of already interned data.
        if let Some(&id) = shard.key_map.find(hash, eq) {
            let value = found_value
                .get()
                .expect("found the interned value, so `found_value` should be set");

            let index = self.database_key_index(id);

            // SAFETY: We hold the lock for the shard containing the value.
            let value_shared = unsafe { &mut *value.shared.get() };

            // Validate the value in this revision to avoid reuse.
            if { value_shared.last_interned_at } < current_revision {
                value_shared.last_interned_at = current_revision;

                zalsa.event(&|| {
                    Event::new(EventKind::DidValidateInternedValue {
                        key: index,
                        revision: current_revision,
                    })
                });

                if value_shared.is_reusable::<C>() {
                    // Move the value to the front of the LRU list.
                    //
                    // SAFETY: We hold the lock for the shard containing the value, and `value` is
                    // a reusable value that was previously interned, so is in the list.
                    unsafe { shard.lru.cursor_mut_from_ptr(value).remove() };

                    // SAFETY: The value pointer is valid for the lifetime of the database
                    // and never accessed mutably directly.
                    unsafe { shard.lru.push_front(UnsafeRef::from_raw(value)) };
                }
            }

            if let Some((_, stamp)) = zalsa_local.active_query() {
                let was_reusable = value_shared.is_reusable::<C>();

                // Record the maximum durability across all queries that intern this value.
                value_shared.durability = std::cmp::max(value_shared.durability, stamp.durability);

                // If the value is no longer reusable, i.e. the durability increased, remove it
                // from the LRU.
                if was_reusable && !value_shared.is_reusable::<C>() {
                    // SAFETY: We hold the lock for the shard containing the value, and `value`
                    // was previously reusable, so is in the list.
                    unsafe { shard.lru.cursor_mut_from_ptr(value).remove() };
                }
            }

            // Record a dependency on the value.
            //
            // See `intern_id_cold` for why we need to use `current_revision` here. Note that just
            // because this value was previously interned does not mean it was previously interned
            // by *our query*, so the same considerations apply.
            zalsa_local.report_tracked_read_simple(
                index,
                value_shared.durability,
                current_revision,
            );

            return value_shared.id;
        }

        // Fill up the table for the first few revisions without attempting garbage collection.
        if !self.revision_queue.is_primed() {
            return self.intern_id_cold(
                key,
                zalsa,
                zalsa_local,
                assemble,
                shard,
                shard_index,
                hash,
            );
        }

        // Otherwise, try to reuse a stale slot.
        let mut cursor = shard.lru.back_mut();

        while let Some(value) = cursor.get() {
            // SAFETY: We hold the lock for the shard containing the value.
            let value_shared = unsafe { &mut *value.shared.get() };

            // The value must not have been read in the current revision to be collected
            // soundly, but we also do not want to collect values that have been read recently.
            //
            // Note that the list is sorted by LRU, so if the tail of the list is not stale, we
            // will not find any stale slots.
            if !self.revision_queue.is_stale(value_shared.last_interned_at) {
                break;
            }

            // We should never reuse a value that was accessed in the current revision.
            debug_assert!({ value_shared.last_interned_at } < current_revision);

            // Record the durability of the current query on the interned value.
            let (durability, last_interned_at) = zalsa_local
                .active_query()
                .map(|(_, stamp)| (stamp.durability, current_revision))
                // If there is no active query this durability does not actually matter.
                // `last_interned_at` needs to be `Revision::MAX`, see the `intern_access_in_different_revision` test.
                .unwrap_or((Durability::MAX, Revision::max()));

            let old_id = value_shared.id;

            // Increment the generation of the ID, as if we allocated a new slot.
            //
            // If the ID is at its maximum generation, we are forced to leak the slot.
            let Some(new_id) = value_shared.id.next_generation() else {
                // Remove the value from the LRU list as we will never be able to
                // collect it.
                cursor.remove().unwrap();

                // Retry with the previous element.
                cursor = shard.lru.back_mut();

                continue;
            };

            // Mark the slot as reused.
            *value_shared = ValueShared {
                id: new_id,
                durability,
                last_interned_at,
            };

            let index = self.database_key_index(value_shared.id);

            // Record a dependency on the new value.
            //
            // See `intern_id_cold` for why we need to use `current_revision` here.
            zalsa_local.report_tracked_read_simple(
                index,
                value_shared.durability,
                current_revision,
            );

            zalsa.event(&|| {
                Event::new(EventKind::DidReuseInternedValue {
                    key: index,
                    revision: current_revision,
                })
            });

            // Remove the value from the LRU list.
            //
            // SAFETY: The value pointer is valid for the lifetime of the database.
            let value = unsafe { &*UnsafeRef::into_raw(cursor.remove().unwrap()) };

            // SAFETY: We hold the lock for the shard containing the value, and the
            // value has not been interned in the current revision, so no references to
            // it can exist.
            let old_fields = unsafe { &mut *value.fields.get() };

            // Remove the previous value from the ID map.
            //
            // Note that while the ID stays the same when a slot is reused, the fields,
            // and thus the hash, will change, so we need to re-insert the value into the
            // map. Crucially, we know that the hashes for the old and new fields both map
            // to the same shard, because we determined the initial shard based on the new
            // fields and only accessed the LRU list for that shard.
            let old_hash = self.hasher.hash_one(&*old_fields);
            shard
                .key_map
                .find_entry(old_hash, |found_id: &Id| *found_id == old_id)
                .expect("interned value in LRU so must be in key_map")
                .remove();

            // Update the fields.
            //
            // SAFETY: We call `from_internal_data` to restore the correct lifetime before access.
            *old_fields = unsafe { self.to_internal_data(assemble(new_id, key)) };

            // SAFETY: We hold the lock for the shard containing the value.
            let hasher = |id: &_| unsafe { self.value_hash(*id, zalsa) };

            // Insert the new value into the ID map.
            shard.key_map.insert_unique(hash, new_id, hasher);

            // SAFETY: We hold the lock for the shard containing the value, and the
            // value has not been interned in the current revision, so no references to
            // it can exist.
            let memo_table = unsafe { &mut *value.memos.get() };

            // Free the memos associated with the previous interned value.
            //
            // SAFETY: The memo table belongs to a value that we allocated, so it has the
            // correct type.
            unsafe { self.clear_memos(zalsa, memo_table, new_id) };

            if value_shared.is_reusable::<C>() {
                // Move the value to the front of the LRU list.
                //
                // SAFETY: The value pointer is valid for the lifetime of the database.
                // and never accessed mutably directly.
                shard.lru.push_front(unsafe { UnsafeRef::from_raw(value) });
            }

            return new_id;
        }

        // If we could not find any stale slots, we are forced to allocate a new one.
        self.intern_id_cold(key, zalsa, zalsa_local, assemble, shard, shard_index, hash)
    }

    /// The cold path for interning a value, allocating a new slot.
    ///
    /// Returns `true` if the current thread interned the value.
    #[allow(clippy::too_many_arguments)]
    fn intern_id_cold<'db, Key>(
        &'db self,
        key: Key,
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        assemble: impl FnOnce(Id, Key) -> C::Fields<'db>,
        shard: &mut IngredientShard<C>,
        shard_index: usize,
        hash: u64,
    ) -> crate::Id
    where
        Key: Hash,
        C::Fields<'db>: HashEqLike<Key>,
    {
        let current_revision = zalsa.current_revision();

        // Record the durability of the current query on the interned value.
        let (durability, last_interned_at) = zalsa_local
            .active_query()
            .map(|(_, stamp)| (stamp.durability, current_revision))
            // If there is no active query this durability does not actually matter.
            // `last_interned_at` needs to be `Revision::MAX`, see the `intern_access_in_different_revision` test.
            .unwrap_or((Durability::MAX, Revision::max()));

        // Allocate the value slot.
        let (id, value) = zalsa_local.allocate(zalsa, self.ingredient_index, |id| Value::<C> {
            shard: shard_index as u16,
            link: LinkedListLink::new(),
            // SAFETY: We only ever access the memos of a value that we allocated through
            // our `MemoTableTypes`.
            memos: UnsafeCell::new(unsafe { MemoTable::new(self.memo_table_types()) }),
            // SAFETY: We call `from_internal_data` to restore the correct lifetime before access.
            fields: UnsafeCell::new(unsafe { self.to_internal_data(assemble(id, key)) }),
            shared: UnsafeCell::new(ValueShared {
                id,
                durability,
                last_interned_at,
            }),
        });

        // Insert the newly allocated ID.
        self.insert_id(id, zalsa, shard, hash, value);

        let index = self.database_key_index(id);

        // Record a dependency on the newly interned value.
        //
        // Note that the ID is unique to this use of the interned slot, so it seems logical to use
        // `Revision::start()` here. However, it is possible that the ID we read is different from
        // the previous execution of this query if the previous slot has been reused. In that case,
        // the query has changed without a corresponding input changing. Using `current_revision`
        // for dependencies on interned values encodes the fact that interned IDs are not stable
        // across revisions.
        zalsa_local.report_tracked_read_simple(index, durability, current_revision);

        zalsa.event(&|| {
            Event::new(EventKind::DidInternValue {
                key: index,
                revision: current_revision,
            })
        });

        id
    }

    /// Inserts a newly interned value ID into the LRU list and key map.
    fn insert_id(
        &self,
        id: Id,
        zalsa: &Zalsa,
        shard: &mut IngredientShard<C>,
        hash: u64,
        value: &Value<C>,
    ) {
        // SAFETY: We hold the lock for the shard containing the value.
        let value_shared = unsafe { &mut *value.shared.get() };

        if value_shared.is_reusable::<C>() {
            // Add the value to the front of the LRU list.
            //
            // SAFETY: The value pointer is valid for the lifetime of the database
            // and never accessed mutably directly.
            shard.lru.push_front(unsafe { UnsafeRef::from_raw(value) });
        }

        // SAFETY: We hold the lock for the shard containing the value.
        let hasher = |id: &_| unsafe { self.value_hash(*id, zalsa) };

        // Insert the value into the ID map.
        shard.key_map.insert_unique(hash, id, hasher);

        debug_assert_eq!(hash, {
            let value = zalsa.table().get::<Value<C>>(id);

            // SAFETY: We hold the lock for the shard containing the value.
            unsafe { self.hasher.hash_one(&*value.fields.get()) }
        });
    }

    /// Clears the given memo table.
    ///
    /// # Safety
    ///
    /// The `MemoTable` must belong to a `Value` of the correct type.
    pub(crate) unsafe fn clear_memos(&self, zalsa: &Zalsa, memo_table: &mut MemoTable, id: Id) {
        // SAFETY: The caller guarantees this is the correct types table.
        let table = unsafe { self.memo_table_types.attach_memos_mut(memo_table) };

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
                    zalsa.ingredient_index_for_memo(self.ingredient_index, memo_ingredient_index);

                let executor = DatabaseKeyIndex::new(ingredient_index, id);

                zalsa.event(&|| Event::new(EventKind::DidDiscard { key: executor }));

                memo.remove_outputs(zalsa, executor);
            })
        };

        std::mem::forget(table_guard);

        // Reset the table after having dropped any memos.
        memo_table.reset();
    }

    // Hashes the value by its fields.
    //
    // # Safety
    //
    // The lock must be held for the shard containing the value.
    unsafe fn value_hash<'db>(&'db self, id: Id, zalsa: &'db Zalsa) -> u64 {
        // This closure is only called if the table is resized. So while it's expensive
        // to lookup all values, it will only happen rarely.
        let value = zalsa.table().get::<Value<C>>(id);

        // SAFETY: We hold the lock for the shard containing the value.
        unsafe { self.hasher.hash_one(&*value.fields.get()) }
    }

    // Compares the value by its fields to the given key.
    //
    // # Safety
    //
    // The lock must be held for the shard containing the value.
    unsafe fn value_eq<'db, Key>(
        id: Id,
        key: &Key,
        zalsa: &'db Zalsa,
        found_value: &Cell<Option<&'db Value<C>>>,
    ) -> bool
    where
        C::Fields<'db>: HashEqLike<Key>,
    {
        let value = zalsa.table().get::<Value<C>>(id);
        found_value.set(Some(value));

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
            {
                let _shard = self.shards[value.shard as usize].lock();

                // SAFETY: We hold the lock for the shard containing the value.
                let value_shared = unsafe { &mut *value.shared.get() };

                let last_changed_revision = zalsa.last_changed_revision(value_shared.durability);
                ({ value_shared.last_interned_at }) >= last_changed_revision
            },
            "Data was not interned in the latest revision for its durability."
        );

        // SAFETY: Interned values are only exposed if they have been validated in the
        // current revision, as checked by the assertion above, which ensures that they
        // are not reused while being accessed.
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
            if should_lock {
                // SAFETY: `value.shard` is guaranteed to be in-bounds for `self.shards`.
                let _shard = unsafe { self.shards.get_unchecked(value.shard as usize) }.lock();
            }

            // SAFETY: The caller guarantees we hold the lock for the shard containing the value.
            //
            // Note that this ID includes the generation, unlike the ID provided by the table.
            let id = unsafe { (*value.shared.get()).id };

            StructEntry {
                value,
                key: self.database_key_index(id),
            }
        })
    }
}

/// An interned struct entry.
pub struct StructEntry<'db, C>
where
    C: Configuration,
{
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
        _cycle_heads: &mut VerifyCycleHeads,
    ) -> VerifyResult {
        // Record the current revision as active.
        let current_revision = zalsa.current_revision();
        self.revision_queue.record(current_revision);

        let value = zalsa.table().get::<Value<C>>(input);

        // SAFETY: `value.shard` is guaranteed to be in-bounds for `self.shards`.
        let _shard = unsafe { self.shards.get_unchecked(value.shard as usize) }.lock();

        // SAFETY: We hold the lock for the shard containing the value.
        let value_shared = unsafe { &mut *value.shared.get() };

        // The slot was reused.
        if value_shared.id.generation() > input.generation() {
            return VerifyResult::changed();
        }

        // Validate the value for the current revision to avoid reuse.
        value_shared.last_interned_at = current_revision;

        zalsa.event(&|| {
            let index = self.database_key_index(input);

            Event::new(EventKind::DidValidateInternedValue {
                key: index,
                revision: current_revision,
            })
        });

        // Any change to an interned value results in a new ID generation.
        VerifyResult::unchanged()
    }

    fn collect_minimum_serialized_edges(
        &self,
        _zalsa: &Zalsa,
        edge: QueryEdge,
        serialized_edges: &mut FxIndexSet<QueryEdge>,
        _visited_edges: &mut FxHashSet<QueryEdge>,
    ) {
        if C::PERSIST && C::REVISIONS != IMMORTAL {
            // If the interned struct is being persisted, it may be reachable through transitive queries.
            // Additionally, interned struct dependencies are impure in that garbage collection can
            // invalidate a dependency without a base input necessarily being updated. Thus, we must
            // preserve the transitive dependency on the interned struct, if garbage collection is
            // enabled.
            serialized_edges.insert(edge);
        }

        // Otherwise, the dependency is covered by the base inputs.
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
    unsafe fn memos(&self, _current_revision: Revision) -> &MemoTable {
        // SAFETY: The fact that we have a reference to the `Value` means it must
        // have been interned, and thus validated, in the current revision.
        unsafe { &*self.memos.get() }
    }

    #[inline(always)]
    fn memos_mut(&mut self) -> &mut MemoTable {
        self.memos.get_mut()
    }
}

/// Keep track of revisions in which interned values were read, to determine staleness.
///
/// An interned value is considered stale if it has not been read in the past `REVS`
/// revisions. However, we only consider revisions in which interned values were actually
/// read, as revisions may be created in bursts.
struct RevisionQueue<C> {
    lock: Mutex<()>,
    // Once `feature(generic_const_exprs)` is stable this can just be an array.
    revisions: Box<[AtomicRevision]>,
    _configuration: PhantomData<fn() -> C>,
}

// `#[salsa::interned(revisions = usize::MAX)]` disables garbage collection.
const IMMORTAL: NonZeroUsize = NonZeroUsize::MAX;

impl<C: Configuration> Default for RevisionQueue<C> {
    fn default() -> RevisionQueue<C> {
        let revisions = if C::REVISIONS == IMMORTAL {
            Box::default()
        } else {
            (0..C::REVISIONS.get())
                .map(|_| AtomicRevision::start())
                .collect()
        };

        RevisionQueue {
            lock: Mutex::new(()),
            revisions,
            _configuration: PhantomData,
        }
    }
}

impl<C: Configuration> RevisionQueue<C> {
    /// Record the given revision as active.
    #[inline]
    fn record(&self, revision: Revision) {
        // Garbage collection is disabled.
        if C::REVISIONS == IMMORTAL {
            return;
        }

        // Fast-path: We already recorded this revision.
        if self.revisions[0].load() >= revision {
            return;
        }

        self.record_cold(revision);
    }

    #[cold]
    fn record_cold(&self, revision: Revision) {
        let _lock = self.lock.lock();

        // Otherwise, update the queue, maintaining sorted order.
        //
        // Note that this should only happen once per revision.
        for i in (1..C::REVISIONS.get()).rev() {
            self.revisions[i].store(self.revisions[i - 1].load());
        }

        self.revisions[0].store(revision);
    }

    /// Returns `true` if the given revision is old enough to be considered stale.
    #[inline]
    fn is_stale(&self, revision: Revision) -> bool {
        // Garbage collection is disabled.
        if C::REVISIONS == IMMORTAL {
            return false;
        }

        let oldest = self.revisions[C::REVISIONS.get() - 1].load();

        // If we have not recorded `REVS` revisions yet, nothing can be stale.
        if oldest == Revision::start() {
            return false;
        }

        revision < oldest
    }

    /// Returns `true` if `C::REVISIONS` revisions have been recorded as active,
    /// i.e. enough data has been recorded to start garbage collection.
    #[inline]
    fn is_primed(&self) -> bool {
        // Garbage collection is disabled.
        if C::REVISIONS == IMMORTAL {
            return false;
        }

        self.revisions[C::REVISIONS.get() - 1].load() > Revision::start()
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

impl HashEqLike<&str> for String {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h)
    }

    fn eq(&self, data: &&str) -> bool {
        self == *data
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

#[cfg(feature = "persistence")]
mod persistence {
    use std::cell::UnsafeCell;
    use std::fmt;
    use std::hash::BuildHasher;

    use intrusive_collections::LinkedListLink;
    use serde::ser::{SerializeMap, SerializeStruct};
    use serde::{de, Deserialize};

    use super::{Configuration, IngredientImpl, Value, ValueShared};
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
                let id = unsafe { (*value.shared.get()).id };

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
                shared,
                shard: _,
                link: _,
                memos: _,
            } = self;

            // SAFETY: The safety invariant of `Ingredient::serialize` ensures we have exclusive access
            // to the database.
            let fields = unsafe { &*fields.get() };

            // SAFETY: The safety invariant of `Ingredient::serialize` ensures we have exclusive access
            // to the database.
            let ValueShared {
                durability,
                last_interned_at,
                id: _,
            } = unsafe { *shared.get() };

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
            C::serialize(self.0, serializer)
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
                    link: LinkedListLink::new(),
                    // SAFETY: We only ever access the memos of a value that we allocated through
                    // our `MemoTableTypes`.
                    memos: UnsafeCell::new(unsafe {
                        MemoTable::new(ingredient.memo_table_types())
                    }),
                    fields: UnsafeCell::new(value.fields.0),
                    shared: UnsafeCell::new(ValueShared {
                        id,
                        durability: value.durability,
                        last_interned_at: value.last_interned_at,
                    }),
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

                // Insert the newly allocated ID into our ingredient.
                ingredient.insert_id(id, zalsa, shard, hash, value);
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
