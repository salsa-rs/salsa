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
use crate::revision::AtomicRevision;
use crate::table::memo::{MemoTable, MemoTableTypes};
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

/// The interned ingredient hashes values of type `Data` to produce an `Id`.
///
/// It used to store interned structs but also to store the id fields of a tracked struct.
/// Interned values endure until they are explicitly removed in some way.
pub struct IngredientImpl<C: Configuration> {
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,

    hasher: FxBuildHasher,

    revision_queue: RevisionQueue,

    shared: Mutex<IngredientImplShared<C>>,

    memo_table_types: Arc<MemoTableTypes>,

    _marker: PhantomData<fn() -> C>,
}

struct IngredientImplShared<C: Configuration> {
    /// Maps from data to the existing interned id for that data.
    ///
    /// This doesn't hold the fields themselves to save memory, instead it points to the slot ID.
    key_map: hashbrown::HashTable<Id>,

    /// An intrusive linked list for LRU.
    lru: LinkedList<ValueAdapter<C>>,
}

// SAFETY: `LinkedListLink` is `!Sync`, however, the linked list is only accessed through the
// ingredient lock, and values are only ever linked to a single list.
unsafe impl<C: Configuration> Sync for Value<C> {}

intrusive_adapter!(ValueAdapter<C> = UnsafeRef<Value<C>>: Value<C> { link: LinkedListLink } where C: Configuration);

/// Struct storing the interned fields.
pub struct Value<C>
where
    C: Configuration,
{
    /// Memos attached to this interned value.
    memos: MemoTable,

    /// An intrusive linked list for LRU.
    link: LinkedListLink,

    /// The interned fields for this value.
    ///
    /// These are valid for read-only access as long as the lock is held
    /// or the value has been validated in the current revision.
    fields: UnsafeCell<C::Fields<'static>>,

    /// Fields that can only be accessed holding the lock.
    shared: UnsafeCell<ValueShared>,
}

/// Shared value fields can only be read through the lock.
struct ValueShared {
    /// The interned ID for this value.
    ///
    /// This is necessary to identify slots in the LRU list.
    id: Id,

    /// The revision the value was first interned in.
    first_interned_at: Revision,

    /// The most recent interned revision.
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
        &*self.fields.with(|fields| unsafe { &*fields })
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
            revision_queue: RevisionQueue::new(),
            shared: Mutex::new(IngredientImplShared {
                key_map: Default::default(),
                lru: LinkedList::default(),
            }),
            memo_table_types: Arc::new(MemoTableTypes::default()),
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
        let table = zalsa.table();

        // Record the current revision as active.
        self.revision_queue.record(current_revision);

        let data_hash = self.hasher.hash_one(&key);

        let mut shared = self.shared.lock();

        let found_value = Cell::new(None);
        let eq = |id: &_| {
            let data = table.get::<Value<C>>(*id);

            found_value.set(Some(data));

            data.fields.with(|fields| {
                // SAFETY: We hold the lock.
                let fields = unsafe { &*fields };

                // SAFETY: it's safe to go from Data<'static> to Data<'db>
                // shrink lifetime here to use a single lifetime in Lookup::eq(&StructKey<'db>, &C::Data<'db>)
                let data = unsafe { Self::from_internal_data(fields) };

                HashEqLike::eq(data, &key)
            })
        };

        // Attempt a fast-path lookup of already interned data.
        if let Some(&id) = shared.key_map.find(data_hash, eq) {
            let value = found_value
                .get()
                .expect("found the interned, so `found_value` should be set");

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

                        // SAFETY: The value pointer is valid for the lifetime of the database
                        // and only accessed mutably while holding the lock.
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
        if !self.revision_queue.is_primed() {
            return self.intern_id_cold(
                db,
                key,
                (zalsa, zalsa_local),
                assemble,
                &mut *shared,
                data_hash,
            );
        }

        // Otherwise, try to reuse a stale slot.
        let mut cursor = shared.lru.back_mut();

        if let Some(value) = cursor.get() {
            let is_stale = value.shared.with(|value_shared| {
                // SAFETY: We hold the lock.
                let last_interned_at = unsafe { (*value_shared).last_interned_at };
                self.revision_queue.is_stale(last_interned_at)
            });

            if is_stale {
                // Record the durability of the current query on the interned value.
                let (durability, last_interned_at) = zalsa_local
                    .active_query()
                    .map(|(_, stamp)| (stamp.durability, current_revision))
                    // If there is no active query this durability does not actually matter.
                    // `last_interned_at` needs to be `Revision::MAX`, see the intern_access_in_different_revision test.
                    .unwrap_or((Durability::MAX, Revision::max()));

                let value = value.shared.get_mut().with(|value_shared| {
                    // SAFETY: We hold the lock.
                    let value_shared = unsafe { &mut *value_shared };

                    // Mark the slot as reused.
                    value_shared.first_interned_at = current_revision;
                    value_shared.last_interned_at = last_interned_at;

                    // Remove the value from the LRU list.
                    //
                    // SAFETY: The value pointer is valid for the lifetime of the database.
                    unsafe { &*UnsafeRef::into_raw(cursor.remove().unwrap()) }
                });

                let id = value.shared.with_mut(|value_shared| {
                    // SAFETY: We hold the lock.
                    let value_shared = unsafe { &mut *value_shared };

                    // Note we need to retain the previous durability here to ensure queries trying
                    // to read the old value are revalidated.
                    value_shared.durability = std::cmp::max(value_shared.durability, durability);

                    let index = self.database_key_index(value_shared.id);

                    // Record a dependency on the value.
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

                // Reuse the value slot with the new data.
                //
                // SAFETY: We hold the lock and marked the value as reused, so any readers in the
                // current revision will see it is not valid.
                value.fields.with_mut(|fields| unsafe {
                    *fields = self.to_internal_data(assemble(id, key));
                });

                // TODO: Need to free the memory safely here.
                value.memos.clear();

                // Move the value to the front of the LRU list.
                //
                // SAFETY: The value pointer is valid for the lifetime of the database
                // and only accessed mutably while holding the lock.
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
            // `last_interned_at` needs to be `Revision::MAX`, see the intern_access_in_different_revision test.
            .unwrap_or((Durability::MAX, Revision::max()));

        // Allocate the value slot.
        let id = zalsa_local.allocate(zalsa, self.ingredient_index, |id| Value::<C> {
            memos: Default::default(),
            link: LinkedListLink::new(),
            fields: UnsafeCell::new(unsafe { self.to_internal_data(assemble(id, key)) }),
            shared: UnsafeCell::new(ValueShared {
                id,
                durability,
                last_interned_at,
                // Record the revision we are interning in.
                first_interned_at: current_revision,
            }),
        });

        let value = zalsa.table().get::<Value<C>>(id);

        // Add the value to the front of the LRU list.
        //
        // SAFETY: The value pointer is valid for the lifetime of the database
        // and only accessed mutably while holding the lock.
        shared.lru.push_front(unsafe { UnsafeRef::from_raw(value) });

        // Insert the value into the ID map.
        let hasher = |id: &_| {
            // This closure is only called if the table is resized. So while it's expensive
            // to lookup all values, it will only happen rarely.
            let value = zalsa.table().get::<Value<C>>(*id);

            // SAFETY: We hold the lock.
            value
                .fields
                .with(|fields| unsafe { self.hasher.hash_one(&*fields) })
        };

        shared.key_map.insert_unique(data_hash, id, hasher);

        debug_assert_eq!(data_hash, {
            let value = zalsa.table().get::<Value<C>>(id);

            // SAFETY: We hold the lock.
            value
                .fields
                .with(|fields| unsafe { self.hasher.hash_one(&*fields) })
        });

        let index = self.database_key_index(id);

        // SAFETY: We hold the lock.
        value.shared.with_mut(|value_shared| {
            // Record a dependency on this value.
            let first_interned_at = unsafe { (*value_shared).first_interned_at };
            zalsa_local.report_tracked_read_simple(index, durability, first_interned_at);
        });

        zalsa.event(&|| {
            Event::new(EventKind::DidInternValue {
                key: index,
                revision: current_revision,
            })
        });

        id
    }

    /// Returns the database key index for an interned value with the given id.
    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.ingredient_index, id)
    }

    /// Lookup the data for an interned value based on its id.
    /// Rarely used since end-users generally carry a struct with a pointer directly
    /// to the interned item.
    pub fn data<'db>(&'db self, db: &'db dyn Database, id: Id) -> &'db C::Fields<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        let value = zalsa.table().get::<Value<C>>(id);

        {
            let _shared = self.shared.lock();

            value.shared.with(|value_shared| {
                // SAFETY: We hold the lock.
                let value_shared = unsafe { &*value_shared };

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
        // current revision, as checked by the assertion above, while ensures they are
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

        // Record the current revision as active.
        self.revision_queue.record(current_revision);

        let _lock = self.shared.lock();

        // SAFETY: We hold the lock.
        value.shared.with_mut(|value_shared| unsafe {
            // The slot was reused.
            if (*value_shared).first_interned_at > revision {
                return VerifyResult::Changed;
            }

            // Validate the value for the current revision to avoid reuse.
            (*value_shared).last_interned_at = current_revision;

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
        &self.memos
    }

    #[inline(always)]
    fn memos_mut(&mut self) -> &mut MemoTable {
        &mut self.memos
    }
}

const REVS: usize = 3;

/// Keep track of revisions in which interned values were read, to determine staleness.
///
/// An interned value is considered stale if it has not been read in the past `REVS`
/// revisions. However, we only consider revisions in which interned values were actually
/// read, as revisions may be created in bursts.
struct RevisionQueue {
    revisions: [AtomicRevision; REVS],
    lock: Mutex<()>,
}

impl RevisionQueue {
    fn new() -> RevisionQueue {
        RevisionQueue {
            revisions: [const { AtomicRevision::start() }; REVS],
            lock: Mutex::default(),
        }
    }

    fn record(&self, revision: Revision) {
        // Fast-path: We already recorded this revision.
        if self.revisions[0].load() >= revision {
            return;
        }

        let mut _revisions = self.lock.lock();

        // Otherwise, update the queue, maintaining sorted order.
        //
        // Note that while this looks expensive, it should only happen
        // once per revision.
        for i in (1..REVS).rev() {
            self.revisions[i].store(self.revisions[i - 1].load());
        }

        self.revisions[0].store(revision);
    }

    fn is_primed(&self) -> bool {
        self.revisions[REVS - 1].load() > Revision::start()
    }

    fn is_stale(&self, revision: Revision) -> bool {
        let oldest = self.revisions[REVS - 1].load();

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
