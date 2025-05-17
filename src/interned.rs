#![allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety

use std::any::TypeId;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink, UnsafeRef};
use parking_lot::Mutex;
use rustc_hash::FxBuildHasher;

use crate::cycle::CycleHeads;
use crate::durability::Durability;
use crate::function::VerifyResult;
use crate::id::{AsId, FromId};
use crate::ingredient::Ingredient;
use crate::loom::cell::{Cell, UnsafeCell};
use crate::loom::sync::Arc;
use crate::plumbing::{IngredientIndices, Jar, ZalsaLocal};
use crate::table::memo::{MemoTable, MemoTableTypes, MemoTableWithTypesMut};
use crate::table::Slot;
use crate::zalsa::{IngredientIndex, Zalsa};
use crate::{Database, DatabaseKeyIndex, Event, EventKind, Id, Revision};

pub trait Configuration: Sized + 'static {
    const LOCATION: crate::ingredient::Location;

    const DEBUG_NAME: &'static str;

    /// The fields of the struct being interned.
    type Fields<'db>: InternedData;

    /// The end user struct
    type Struct<'db>: Copy + FromId + AsId;
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

    /// A hasher for the shared ID map.
    hasher: FxBuildHasher,

    /// Shared data that can only be accessed through a lock.
    shared: Mutex<IngredientImplShared<C>>,

    memo_table_types: Arc<MemoTableTypes>,

    _marker: PhantomData<fn() -> C>,
}

struct IngredientImplShared<C: Configuration> {
    /// Maps from data to the existing interned ID for that data.
    ///
    /// This doesn't hold the fields themselves to save memory, instead it points
    /// to the slot ID.
    key_map: hashbrown::HashTable<Id>,

    /// An intrusive linked list for LRU.
    lru: LinkedList<ValueAdapter<C>>,

    /// A queue of recent revisions in which values were interned.
    revision_queue: RevisionQueue,
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

    /// Fields that can only be accessed holding the lock.
    shared: UnsafeCell<ValueShared>,
}

/// Shared value fields can only be read through the lock.
struct ValueShared {
    /// The interned ID for this value.
    ///
    /// Storing this on the value itself is necessary to identify slots
    /// from the LRU list.
    id: Id,

    /// The revision the value was first interned in.
    first_interned_at: Revision,

    /// The revision the value was most-recently interned in.
    last_interned_at: Revision,

    /// The minimum durability of all inputs consumed by the creator
    /// query prior to creating this tracked struct. If any of those
    /// inputs changes, then the creator query may create this struct
    /// with different values.
    durability: Durability,
}

impl<C> Value<C>
where
    C: Configuration,
{
    /// Fields of this interned struct.
    #[cfg(feature = "salsa_unstable")]
    pub fn fields(&self) -> &C::Fields<'static> {
        // SAFETY: The fact that this function is safe is technically unsound, but interned
        // values are only exposed if they have been validated in the current revision, which
        // ensures that they are not reused while being accessed.
        self.fields.with(|fields| unsafe { &*fields })
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
        _zalsa: &Zalsa,
        first_index: IngredientIndex,
        _dependencies: IngredientIndices,
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
        Self {
            ingredient_index,
            hasher: FxBuildHasher,
            memo_table_types: Arc::new(MemoTableTypes::default()),
            shared: Mutex::new(IngredientImplShared {
                key_map: hashbrown::HashTable::default(),
                lru: LinkedList::default(),
                revision_queue: RevisionQueue::default(),
            }),
            _marker: PhantomData,
        }
    }

    unsafe fn to_internal_data<'db>(&'db self, data: C::Fields<'db>) -> C::Fields<'static> {
        unsafe { std::mem::transmute(data) }
    }

    unsafe fn from_internal_data<'db>(data: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
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
        db: &'db dyn crate::Database,
        key: Key,
        assemble: impl FnOnce(Id, Key) -> C::Fields<'db>,
    ) -> C::Struct<'db>
    where
        Key: Hash,
        C::Fields<'db>: HashEqLike<Key>,
    {
        FromId::from_id(self.intern_id(db, key, assemble))
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
        db: &'db dyn crate::Database,
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
        let (zalsa, zalsa_local) = db.zalsas();
        let current_revision = zalsa.current_revision();

        let data_hash = self.hasher.hash_one(&key);

        let mut shared = self.shared.lock();

        // Record the current revision as active.
        shared.revision_queue.record(current_revision);

        let found_value = Cell::new(None);

        // SAFETY: We hold the lock.
        let eq = |id: &_| unsafe { Self::value_eq(*id, &key, zalsa, &found_value) };

        // Attempt a fast-path lookup of already interned data.
        if let Some(&id) = shared.key_map.find(data_hash, eq) {
            let value = found_value
                .get()
                .expect("found the interned value, so `found_value` should be set");

            let index = self.database_key_index(id);

            let id = value.shared.with_mut(|value_shared| {
                // SAFETY: We hold the lock.
                let value_shared = unsafe { &mut *value_shared };

                // Validate the value in this revision to avoid reuse.
                if value_shared.last_interned_at < current_revision {
                    value_shared.last_interned_at = current_revision;

                    zalsa.event(&|| {
                        Event::new(EventKind::DidValidateInternedValue {
                            key: index,
                            revision: current_revision,
                        })
                    });

                    // Move the value to the front of the LRU list.
                    unsafe {
                        // SAFETY: We hold the lock and `value` was previously interned, so is
                        // in the list.
                        shared.lru.cursor_mut_from_ptr(value).remove();

                        // SAFETY: The value pointer is valid for the lifetime of the database.
                        // and never accessed mutably directly.
                        shared.lru.push_front(UnsafeRef::from_raw(value));
                    }
                }

                // Record the maximum durability across all queries that intern this value.
                if let Some((_, stamp)) = zalsa_local.active_query() {
                    value_shared.durability =
                        std::cmp::max(value_shared.durability, stamp.durability);
                }

                // Record a dependency on the value.
                zalsa_local.report_tracked_read_simple(
                    index,
                    value_shared.durability,
                    value_shared.first_interned_at,
                );

                id
            });

            return id;
        }

        // Fill up the table for the first few revisions.
        if !shared.revision_queue.is_primed() {
            return self.intern_id_cold(
                db,
                key,
                (zalsa, zalsa_local),
                assemble,
                &mut *shared,
                data_hash,
            );
        }

        let IngredientImplShared {
            lru,
            revision_queue,
            ..
        } = &mut *shared;

        // Otherwise, try to reuse a stale slot.
        let mut cursor = lru.back_mut();

        if let Some(value) = cursor.get() {
            let is_stale = value.shared.with(|value_shared| {
                // SAFETY: We hold the lock.
                let value_shared = unsafe { &*value_shared };

                // The value must not have been read in the current revision to be collected
                // safely, but we also do not want to collect values that have been read recently.
                let is_stale = revision_queue.is_stale(value_shared.last_interned_at);

                // We can't collect higher durability values until we have a mutable reference to the database.
                let is_low_durability = value_shared.durability == Durability::LOW;

                is_stale && is_low_durability
            });

            if is_stale {
                // Record the durability of the current query on the interned value.
                let (durability, last_interned_at) = zalsa_local
                    .active_query()
                    .map(|(_, stamp)| (stamp.durability, current_revision))
                    // If there is no active query this durability does not actually matter.
                    // `last_interned_at` needs to be `Revision::MAX`, see the `intern_access_in_different_revision` test.
                    .unwrap_or((Durability::MAX, Revision::max()));

                let id = value.shared.get_mut().with(|value_shared| {
                    // SAFETY: We hold the lock.
                    let value_shared = unsafe { &mut *value_shared };

                    // Mark the slot as reused.
                    *value_shared = ValueShared {
                        durability,
                        last_interned_at,
                        id: value_shared.id,
                        // Record the revision in which we are re-interning the value.
                        first_interned_at: current_revision,
                    };

                    // Record a dependency on the value.
                    let index = self.database_key_index(value_shared.id);
                    zalsa_local.report_tracked_read_simple(
                        index,
                        value_shared.durability,
                        value_shared.first_interned_at,
                    );

                    zalsa.event(&|| {
                        Event::new(EventKind::DidReuseInternedValue {
                            key: index,
                            revision: current_revision,
                        })
                    });

                    value_shared.id
                });

                // Remove the value from the LRU list.
                //
                // SAFETY: The value pointer is valid for the lifetime of the database.
                let value = unsafe { &*UnsafeRef::into_raw(cursor.remove().unwrap()) };

                // Reuse the value slot with the new data.
                value.fields.with_mut(|old_fields| {
                    // SAFETY: We hold the lock.
                    let old_data_hash = unsafe { self.hasher.hash_one(&*old_fields) };

                    // Remove the previous value from the ID map.
                    //
                    // Note that while the ID stays the same when a slot is reused, the fields,
                    // and thus the hash, will change.
                    shared
                        .key_map
                        .find_entry(old_data_hash, |found_id: &Id| *found_id == id)
                        .expect("interned value in LRU so must be in key_map")
                        .remove();

                    // Update the fields.
                    //
                    // SAFETY: We hold the lock and marked the value as reused, so any
                    // readers in the current revision will see that it is not valid.
                    unsafe { *old_fields = self.to_internal_data(assemble(id, key)) };

                    // SAFETY: We hold the lock.
                    let hasher = |id: &_| unsafe { self.value_hash(*id, zalsa) };

                    // Insert the new value into the ID map.
                    shared.key_map.insert_unique(data_hash, id, hasher);
                });

                // Free the memos associated with the previous interned value.
                let mut memo_table = value.memos.with_mut(|memos| {
                    // SAFETY: We hold the lock, and the value has not been interned
                    // in the current revision, so no references to it can exist.
                    unsafe { std::mem::take(&mut *memos) }
                });

                // SAFETY: We use the correct types table.
                let table = unsafe { self.memo_table_types.attach_memos_mut(&mut memo_table) };

                // `Database::salsa_event` is a user supplied callback which may panic.
                // In that case we need a drop guard to free the memo table.
                struct TableDropGuard<'a>(MemoTableWithTypesMut<'a>);

                impl Drop for TableDropGuard<'_> {
                    fn drop(&mut self) {
                        // SAFETY: We have verified that no more references to these memos
                        // exist and so we are good to drop them.
                        unsafe { self.0.drop() };
                    }
                }

                let mut table_guard = TableDropGuard(table);

                // SAFETY: We have verified that no more references to these memos exist
                // and so we are good to drop them.
                unsafe {
                    table_guard.0.take_memos(|memo_ingredient_index, memo| {
                        let ingredient_index = zalsa.ingredient_index_for_memo(
                            self.ingredient_index,
                            memo_ingredient_index,
                        );

                        let executor = DatabaseKeyIndex::new(ingredient_index, id);

                        zalsa.event(&|| Event::new(EventKind::DidDiscard { key: executor }));

                        for stale_output in memo.origin().outputs() {
                            stale_output.remove_stale_output(zalsa, executor);
                        }
                    })
                };

                std::mem::forget(table_guard);

                // Move the value to the front of the LRU list.
                //
                // SAFETY: The value pointer is valid for the lifetime of the database.
                // and never accessed mutably directly.
                shared.lru.push_front(unsafe { UnsafeRef::from_raw(value) });

                return id;
            }
        }

        // If we could not find any stale slots, we are forced to allocate a new one.
        self.intern_id_cold(
            db,
            key,
            (zalsa, zalsa_local),
            assemble,
            &mut *shared,
            data_hash,
        )
    }

    /// The cold path for interning a value, allocating a new slot.
    ///
    /// Returns `true` if the current thread interned the value.
    fn intern_id_cold<'db, Key>(
        &'db self,
        _db: &'db dyn crate::Database,
        key: Key,
        (zalsa, zalsa_local): (&Zalsa, &ZalsaLocal),
        assemble: impl FnOnce(Id, Key) -> C::Fields<'db>,
        shared: &mut IngredientImplShared<C>,
        data_hash: u64,
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
        let id = zalsa_local.allocate(zalsa, self.ingredient_index, |id| Value::<C> {
            fields: UnsafeCell::new(unsafe { self.to_internal_data(assemble(id, key)) }),
            shared: UnsafeCell::new(ValueShared {
                id,
                durability,
                last_interned_at,
                // Record the revision in which we are re-interning the value.
                first_interned_at: current_revision,
            }),
            link: LinkedListLink::new(),
            memos: UnsafeCell::new(MemoTable::default()),
        });

        let value = zalsa.table().get::<Value<C>>(id);

        // Add the value to the front of the LRU list.
        //
        // SAFETY: The value pointer is valid for the lifetime of the database
        // and only accessed mutably after being removed from the list.
        shared.lru.push_front(unsafe { UnsafeRef::from_raw(value) });

        // SAFETY: We hold the lock.
        let hasher = |id: &_| unsafe { self.value_hash(*id, zalsa) };

        // Insert the value into the ID map.
        shared.key_map.insert_unique(data_hash, id, hasher);

        debug_assert_eq!(data_hash, {
            let value = zalsa.table().get::<Value<C>>(id);

            // SAFETY: We hold the lock.
            value
                .fields
                .with(|fields| unsafe { self.hasher.hash_one(&*fields) })
        });

        let index = self.database_key_index(id);

        // Record a dependency on the newly interned value.
        zalsa_local.report_tracked_read_simple(index, durability, current_revision);

        zalsa.event(&|| {
            Event::new(EventKind::DidInternValue {
                key: index,
                revision: current_revision,
            })
        });

        id
    }

    // Hashes the value by its fields.
    //
    // # Safety
    //
    // The lock must be held.
    unsafe fn value_hash<'db>(&'db self, id: Id, zalsa: &'db Zalsa) -> u64 {
        // This closure is only called if the table is resized. So while it's expensive
        // to lookup all values, it will only happen rarely.
        let value = zalsa.table().get::<Value<C>>(id);

        // SAFETY: We hold the lock.
        value
            .fields
            .with(|fields| unsafe { self.hasher.hash_one(&*fields) })
    }

    // Compares the value by its fields to the given key.
    //
    // # Safety
    //
    // The lock must be held.
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

        value.fields.with(|fields| {
            // SAFETY: We hold the lock.
            let fields = unsafe { &*fields };

            // SAFETY: It's sound to go from `Data<'static>` to `Data<'db>`. We shrink the
            // lifetime here to use a single lifetime in `Lookup::eq(&StructKey<'db>, &C::Data<'db>)`
            let data = unsafe { Self::from_internal_data(fields) };

            HashEqLike::eq(data, key)
        })
    }

    /// Returns the database key index for an interned value with the given id.
    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.ingredient_index, id)
    }

    /// Lookup the data for an interned value based on its ID.
    pub fn data<'db>(&'db self, db: &'db dyn Database, id: Id) -> &'db C::Fields<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        let value = zalsa.table().get::<Value<C>>(id);

        {
            let _shared = self.shared.lock();

            value.shared.with(|value_shared| {
                // SAFETY: We hold the lock.
                let value_shared = unsafe { &*value_shared };

                // Record a read dependency on the value.
                //
                // This is necessary as interned slots may be reused, in which case any queries
                // that created or read from the previous value need to be revalidated.
                zalsa_local.report_tracked_read_simple(
                    self.database_key_index(id),
                    value_shared.durability,
                    value_shared.first_interned_at,
                );

                let last_changed_revision = zalsa.last_changed_revision(value_shared.durability);

                debug_assert!(
                    value_shared.last_interned_at >= last_changed_revision,
                    "Data was not interned in the latest revision for its durability."
                );
            });
        }

        // SAFETY: Interned values are only exposed if they have been validated in the
        // current revision, as checked by the assertion above, which ensures they are
        // not reused while being accessed.
        value
            .fields
            .with(|fields| unsafe { Self::from_internal_data(&*fields) })
    }

    /// Lookup the fields from an interned struct.
    /// Note that this is not "leaking" since no dependency edge is required.
    pub fn fields<'db>(&'db self, db: &'db dyn Database, s: C::Struct<'db>) -> &'db C::Fields<'db> {
        self.data(db, AsId::as_id(&s))
    }

    pub fn reset(&mut self, db: &mut dyn Database) {
        _ = db.zalsa_mut();
        // We can clear the key_map now that we have cancelled all other handles.
        self.shared.lock().key_map.clear();
    }

    #[cfg(feature = "salsa_unstable")]
    /// Returns all data corresponding to the interned struct.
    pub fn entries<'db>(
        &'db self,
        db: &'db dyn crate::Database,
    ) -> impl Iterator<Item = &'db Value<C>> {
        db.zalsa().table().slots_of::<Value<C>>()
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
        db: &dyn Database,
        input: Id,
        revision: Revision,
        _cycle_heads: &mut CycleHeads,
    ) -> VerifyResult {
        let zalsa = db.zalsa();
        let current_revision = zalsa.current_revision();

        let value = zalsa.table().get::<Value<C>>(input);

        let mut shared = self.shared.lock();

        // Record the current revision as active.
        shared.revision_queue.record(current_revision);

        value.shared.with_mut(|value_shared| {
            // SAFETY: We hold the lock.
            let value_shared = unsafe { &mut *value_shared };

            // The slot was reused.
            if value_shared.first_interned_at > revision {
                return VerifyResult::Changed;
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

            VerifyResult::unchanged()
        })
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
    }

    fn memo_table_types(&self) -> Arc<MemoTableTypes> {
        self.memo_table_types.clone()
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

impl<C> Slot for Value<C>
where
    C: Configuration,
{
    #[inline(always)]
    unsafe fn memos(&self, _current_revision: Revision) -> &MemoTable {
        // SAFETY: The fact that we have a reference to the `Value` means it must
        // have been interned, and thus validated, in the current revision.
        self.memos.with(|memos| unsafe { &*memos })
    }

    #[inline(always)]
    fn memos_mut(&mut self) -> &mut MemoTable {
        // SAFETY: We have `&mut self`.
        self.memos.with_mut(|memos| unsafe { &mut *memos })
    }
}

const REVS: usize = 3;

/// Keep track of revisions in which interned values were read, to determine staleness.
///
/// An interned value is considered stale if it has not been read in the past `REVS`
/// revisions. However, we only consider revisions in which interned values were actually
/// read, as revisions may be created in bursts.
struct RevisionQueue {
    revisions: [Revision; REVS],
}

impl Default for RevisionQueue {
    fn default() -> RevisionQueue {
        RevisionQueue {
            revisions: [const { Revision::start() }; REVS],
        }
    }
}

impl RevisionQueue {
    fn record(&mut self, revision: Revision) {
        // Fast-path: We already recorded this revision.
        if self.revisions[0] >= revision {
            return;
        }

        // Otherwise, update the queue, maintaining sorted order.
        //
        // Note that while this looks expensive, it should only happen
        // once per revision.
        for i in (1..REVS).rev() {
            self.revisions[i] = self.revisions[i - 1];
        }

        self.revisions[0] = revision;
    }

    fn is_primed(&self) -> bool {
        self.revisions[REVS - 1] > Revision::start()
    }

    fn is_stale(&self, revision: Revision) -> bool {
        let oldest = self.revisions[REVS - 1];

        // If we have not recorded three revisions yet, nothing can be stale.
        if oldest == Revision::start() {
            return false;
        }

        revision <= oldest
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
