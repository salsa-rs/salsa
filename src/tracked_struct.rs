#![allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety

use std::any::TypeId;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ops::Index;
use std::{fmt, mem};

use crossbeam_queue::SegQueue;
use hashbrown::hash_table::Entry;
use thin_vec::ThinVec;
use tracked_field::FieldIngredientImpl;

use crate::function::{VerifyCycleHeads, VerifyResult};
use crate::hash::{FxHashSet, FxIndexSet};
use crate::id::{AsId, FromId};
use crate::ingredient::{Ingredient, Jar};
use crate::key::DatabaseKeyIndex;
use crate::plumbing::{self, ZalsaLocal};
use crate::revision::OptionalAtomicRevision;
use crate::runtime::Stamp;
use crate::salsa_struct::SalsaStructInDb;
use crate::sync::Arc;
use crate::table::memo::{MemoTable, MemoTableTypes, MemoTableWithTypesMut};
use crate::table::{Slot, Table};
use crate::zalsa::{IngredientIndex, JarKind, Zalsa};
use crate::zalsa_local::QueryEdge;
use crate::{Durability, Event, EventKind, Id, Revision};

pub mod tracked_field;

// ANCHOR: Configuration
/// Trait that defines the key properties of a tracked struct.
///
/// Implemented by the `#[salsa::tracked]` macro when applied
/// to a struct.
pub trait Configuration: Sized + 'static {
    const LOCATION: crate::ingredient::Location;

    /// The debug name of the tracked struct.
    const DEBUG_NAME: &'static str;

    /// The debug names of any tracked fields.
    const TRACKED_FIELD_NAMES: &'static [&'static str];

    /// The relative indices of any tracked fields.
    const TRACKED_FIELD_INDICES: &'static [usize];

    /// Whether this struct should be persisted with the database.
    const PERSIST: bool;

    /// A (possibly empty) tuple of the fields for this struct.
    type Fields<'db>: Send + Sync;

    /// A array of [`Revision`][] values, one per each of the tracked value fields.
    /// When a struct is re-recreated in a new revision, the corresponding
    /// entries for each field are updated to the new revision if their
    /// values have changed (or if the field is marked as `#[no_eq]`).
    #[cfg(feature = "persistence")]
    type Revisions: Send
        + Sync
        + Index<usize, Output = Revision>
        + plumbing::serde::Serialize
        + for<'de> plumbing::serde::Deserialize<'de>;

    #[cfg(not(feature = "persistence"))]
    type Revisions: Send + Sync + Index<usize, Output = Revision>;

    type Struct<'db>: Copy + FromId + AsId;

    fn untracked_fields(fields: &Self::Fields<'_>) -> impl Hash;

    /// Create a new value revision array where each element is set to `current_revision`.
    fn new_revisions(current_revision: Revision) -> Self::Revisions;

    /// Update the field data and, if the value has changed,
    /// the appropriate entry in the `revisions` array (tracked fields only).
    ///
    /// Returns `true` if any untracked field was updated and
    /// the struct should be considered re-created.
    ///
    /// # Safety
    ///
    /// Requires the same conditions as the `maybe_update`
    /// method on [the `Update` trait](`crate::update::Update`).
    ///
    /// In short, requires that `old_fields` be a pointer into
    /// storage from a previous revision.
    /// It must meet its validity invariant.
    /// Owned content must meet safety invariant.
    /// `*mut` here is not strictly needed;
    /// it is used to signal that the content
    /// is not guaranteed to recursively meet
    /// its safety invariant and
    /// hence this must be dereferenced with caution.
    ///
    /// Ensures that `old_fields` is fully updated and valid
    /// after it returns and that `revisions` has been updated
    /// for any field that changed.
    unsafe fn update_fields<'db>(
        current_revision: Revision,
        revisions: &mut Self::Revisions,
        old_fields: *mut Self::Fields<'db>,
        new_fields: Self::Fields<'db>,
    ) -> bool;

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
// ANCHOR_END: Configuration

pub struct JarImpl<C>
where
    C: Configuration,
{
    phantom: PhantomData<C>,
}

impl<C: Configuration> Default for JarImpl<C> {
    fn default() -> Self {
        Self {
            phantom: Default::default(),
        }
    }
}

impl<C: Configuration> Jar for JarImpl<C> {
    fn create_ingredients(
        _zalsa: &mut Zalsa,
        struct_index: crate::zalsa::IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>> {
        let struct_ingredient = <IngredientImpl<C>>::new(struct_index);

        let tracked_field_ingredients =
            C::TRACKED_FIELD_INDICES
                .iter()
                .copied()
                .map(|tracked_index| {
                    Box::new(<FieldIngredientImpl<C>>::new(
                        tracked_index,
                        struct_index.successor(tracked_index),
                    )) as _
                });

        std::iter::once(Box::new(struct_ingredient) as _)
            .chain(tracked_field_ingredients)
            .collect()
    }

    fn id_struct_type_id() -> TypeId {
        TypeId::of::<C::Struct<'static>>()
    }
}

pub trait TrackedStructInDb: SalsaStructInDb {
    /// Converts the identifier for this tracked struct into a `DatabaseKeyIndex`.
    fn database_key_index(zalsa: &Zalsa, id: Id) -> DatabaseKeyIndex;
}

/// Created for each tracked struct.
///
/// This ingredient only stores the "id" fields. It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked
/// struct id. The value fields are stored in [`crate::function::IngredientImpl`]
/// instances keyed by the tracked struct id.
///
/// Unlike normal interned values, tracked struct indices can be deleted and reused aggressively
/// without dependency edges on the creating query. When a tracked function is collected,
/// any tracked structs it created can be deleted. Additionally, when a tracked function
/// re-executes but does not create a tracked struct that was previously created, it can
/// be deleted. No dependency edge is required as the lifetime of a tracked struct is tied
/// directly to the query that created it.
pub struct IngredientImpl<C>
where
    C: Configuration,
{
    /// Our index in the database.
    ingredient_index: IngredientIndex,

    /// Phantom data: we fetch `Value<C>` out from `Table`
    phantom: PhantomData<fn() -> Value<C>>,

    /// Store freed ids
    free_list: SegQueue<Id>,

    memo_table_types: Arc<MemoTableTypes>,
}

/// Defines the identity of a tracked struct.
/// This is the key to a hashmap that is (initially)
/// stored in the [`ActiveQuery`](`crate::active_query::ActiveQuery`)
/// struct and later moved to the [`Memo`](`crate::function::memo::Memo`).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct Identity {
    // Conceptually, this contains an `IdentityHash`, but using `IdentityHash` directly will grow the size
    // of this struct struct by a `std::mem::size_of::<usize>()` due to unusable padding. To avoid this increase
    // in size, we inline the fields of `IdentityHash`.
    /// Index of the tracked struct ingredient.
    ingredient_index: IngredientIndex,

    /// Hash of the id fields.
    hash: u64,

    /// The unique disambiguator assigned within the active query
    /// to distinguish distinct tracked structs with the same identity_hash.
    disambiguator: Disambiguator,
}

impl Identity {
    pub(crate) fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }
}

/// Stores the data that (almost) uniquely identifies a tracked struct.
///
/// This includes the ingredient index of that struct type plus the hash of its untracked
/// fields. This is mapped to a disambiguator -- a value that starts as 0 but increments
/// each round, allowing for multiple tracked structs with the same hash and `IngredientIndex`
/// created within the query to each have a unique ID.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub struct IdentityHash {
    /// Index of the tracked struct ingredient.
    ingredient_index: IngredientIndex,

    /// Hash of the id fields.
    hash: u64,
}

/// A map from tracked struct [`Identity`] to their final [`Id`].
#[derive(Default, Debug)]
pub(crate) struct IdentityMap {
    // We use a `HashTable` here as our key contains its own hash (`Identity::hash`),
    // so we do the hash wrangling ourselves.
    table: hashbrown::HashTable<TrackedEntry>,
}

impl IdentityMap {
    /// Seeds the identity map with the IDs from a previous revision.
    pub(crate) fn seed(&mut self, source: &[(Identity, Id)]) {
        for &(key, id) in source {
            self.insert_entry(key, id, false);
        }
    }

    // Mark all tracked structs in the map as created by the current query.
    pub(crate) fn mark_all_active(&mut self, items: impl IntoIterator<Item = (Identity, Id)>) {
        for (key, id) in items {
            self.insert_entry(key, id, true);
        }
    }

    /// Insert a tracked struct identity into the map with the given ID.
    pub(crate) fn insert(&mut self, key: Identity, id: Id) -> Option<Id> {
        self.insert_entry(key, id, true)
    }

    fn insert_entry(&mut self, key: Identity, id: Id, active: bool) -> Option<Id> {
        let entry = self.table.entry(
            key.hash,
            |entry| entry.identity == key,
            |entry| entry.identity.hash,
        );

        match entry {
            Entry::Vacant(entry) => {
                entry.insert(TrackedEntry {
                    identity: key,
                    id,
                    active,
                });

                None
            }
            Entry::Occupied(mut entry) => {
                let tracked = entry.get_mut();
                tracked.active = active;

                Some(std::mem::replace(&mut tracked.id, id))
            }
        }
    }

    /// Reuses an existing identity if it already exists in the map, marking it as active.
    ///
    /// Returns the existing ID, or `None` if no ID for the given identity exists.
    pub(crate) fn reuse(&mut self, key: &Identity) -> Option<Id> {
        self.table
            .find_mut(key.hash, |entry| key == &entry.identity)
            .map(|entry| {
                entry.active = true;
                entry.id
            })
    }

    /// Returns `true` if the given tracked struct key was created in the current query execution.
    pub(crate) fn is_active(&self, key: DatabaseKeyIndex) -> bool {
        self.table
            .iter()
            .find(|entry| {
                entry.id == key.key_index()
                    && entry.identity.ingredient_index() == key.ingredient_index()
            })
            .is_some_and(|entry| entry.active)
    }

    /// Drains the [`IdentityMap`] into a tuple of active and stale tracked structs.
    ///
    /// The first entry contains the identity and IDs of any tracked structs that were
    /// created by the current execution of the query, while the second entry contains any
    /// tracked structs that were created in a previous execution but not the current one.
    #[expect(clippy::type_complexity)]
    pub(crate) fn drain(&mut self) -> (ThinVec<(Identity, Id)>, Vec<(Identity, Id)>) {
        if self.table.is_empty() {
            return (ThinVec::new(), Vec::new());
        }

        let mut stale = Vec::new();
        let mut active = ThinVec::with_capacity(self.table.len());

        for entry in self.table.drain() {
            if entry.active {
                active.push((entry.identity, entry.id));
            } else {
                stale.push((entry.identity, entry.id));
            }
        }

        // Removing a stale tracked struct ID shows up in the event logs, so make sure
        // the order is stable here.
        stale.sort_unstable_by(|a, b| {
            (a.0.ingredient_index(), a.1).cmp(&(b.0.ingredient_index(), b.1))
        });

        (active, stale)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.table.clear()
    }
}

/// A tracked struct entry stored in an [`IdentityMap`].
#[derive(Debug)]
struct TrackedEntry {
    /// The identity of the tracked struct.
    identity: Identity,

    /// The current ID of the tracked struct.
    id: Id,

    /// Whether or not this tracked struct was created by the current query.
    ///
    /// Entries where `active` is `false` represent tracked structs that were created
    /// by a previous execution of the query, but not in the current one, and hence can
    /// be collected.
    active: bool,
}

// ANCHOR: ValueStruct
#[derive(Debug)]
pub struct Value<C>
where
    C: Configuration,
{
    /// The minimum durability of all inputs consumed by the creator
    /// query prior to creating this tracked struct. If any of those
    /// inputs changes, then the creator query may create this struct
    /// with different values.
    durability: Durability,

    /// The revision when this tracked struct was last updated.
    /// This field also acts as a kind of "lock". Once it is equal
    /// to `Some(current_revision)`, the fields are locked and
    /// cannot change further. This makes it safe to give out `&`-references
    /// so long as they do not live longer than the current revision
    /// (which is assured by tying their lifetime to the lifetime of an `&`-ref
    /// to the database).
    ///
    /// The struct is updated from an older revision `R0` to the current revision `R1`
    /// when the struct is first accessed in `R1`, whether that be because the original
    /// query re-created the struct (i.e., by user calling `Struct::new`) or because
    /// the struct was read from. (Structs may not be recreated in the new revision if
    /// the inputs to the query have not changed.)
    ///
    /// When re-creating the struct, the field is temporarily set to `None`.
    /// This is signal that there is an active `&mut` modifying the other fields:
    /// even reading from those fields in that situation would create UB.
    /// This `None` value should never be observable by users unless they have
    /// leaked a reference across threads somehow.
    updated_at: OptionalAtomicRevision,

    /// Fields of this tracked struct. They can change across revisions,
    /// but they do not change within a particular revision.
    fields: C::Fields<'static>,

    /// The revision information for each field: when did this field last change.
    /// When tracked structs are re-created, this revision may be updated to the
    /// current revision if the value is different.
    revisions: C::Revisions,

    /// Memo table storing the results of query functions etc.
    /*unsafe */
    memos: MemoTable,
}
// ANCHOR_END: ValueStruct

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
#[cfg_attr(feature = "persistence", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "persistence", serde(transparent))]
pub struct Disambiguator(u32);

#[derive(Default, Debug)]
pub(crate) struct DisambiguatorMap {
    // we use a non-hasher hashmap here as our key contains its own hash (in a sense)
    // so we use the raw entry api instead to avoid the overhead of hashing unnecessarily
    map: hashbrown::HashMap<IdentityHash, Disambiguator, ()>,
}

impl DisambiguatorMap {
    pub(crate) fn disambiguate(&mut self, key: IdentityHash) -> Disambiguator {
        use hashbrown::hash_map::RawEntryMut;

        let entry = self.map.raw_entry_mut().from_hash(key.hash, |k| *k == key);
        let disambiguator = match entry {
            RawEntryMut::Occupied(occupied) => occupied.into_mut(),
            RawEntryMut::Vacant(vacant) => {
                vacant
                    .insert_with_hasher(key.hash, key, Disambiguator(0), |k| k.hash)
                    .1
            }
        };
        let result = *disambiguator;
        disambiguator.0 += 1;
        result
    }

    pub fn clear(&mut self) {
        self.map.clear()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Create a tracked struct ingredient. Generated by the `#[tracked]` macro,
    /// not meant to be called directly by end-users.
    fn new(index: IngredientIndex) -> Self {
        Self {
            ingredient_index: index,
            phantom: PhantomData,
            free_list: Default::default(),
            memo_table_types: Arc::new(MemoTableTypes::default()),
        }
    }

    /// Returns the database key index for a tracked struct with the given id.
    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.ingredient_index, id)
    }

    pub fn new_struct<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        mut fields: C::Fields<'db>,
    ) -> C::Struct<'db> {
        let identity_hash = IdentityHash {
            ingredient_index: self.ingredient_index,
            hash: crate::hash::hash(&C::untracked_fields(&fields)),
        };

        let (current_deps, disambiguator) = zalsa_local.disambiguate(identity_hash);

        let identity = Identity {
            hash: identity_hash.hash,
            ingredient_index: identity_hash.ingredient_index,
            disambiguator,
        };

        let current_revision = zalsa.current_revision();
        if let Some(id) = zalsa_local.tracked_struct_id(&identity) {
            // The struct already exists in the intern map.
            let index = self.database_key_index(id);
            crate::tracing::trace!("Reuse tracked struct {id:?}", id = index);

            // SAFETY: The `id` was present in the interned map, so the value must be initialized.
            let update_result =
                unsafe { self.update(zalsa, current_revision, id, &current_deps, fields) };

            fields = match update_result {
                // Overwrite the previous ID if we are reusing the old slot with new fields.
                Ok(updated_id) if updated_id != id => {
                    zalsa_local.store_tracked_struct_id(identity, updated_id);
                    return FromId::from_id(updated_id);
                }

                // The id has not changed.
                Ok(id) => return FromId::from_id(id),

                // Failed to perform the update, we are forced to allocate a new slot.
                Err(fields) => fields,
            };
        }

        // We failed to perform the update, or this is a new tracked struct, so allocate a new entry
        // in the struct map.
        let id = self.allocate(zalsa, zalsa_local, current_revision, &current_deps, fields);
        let key = self.database_key_index(id);
        crate::tracing::trace!("Allocated new tracked struct {key:?}");
        zalsa_local.store_tracked_struct_id(identity, id);
        FromId::from_id(id)
    }

    fn allocate<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        current_revision: Revision,
        current_deps: &Stamp,
        fields: C::Fields<'db>,
    ) -> Id {
        let value = |_| Value {
            updated_at: OptionalAtomicRevision::new(Some(current_revision)),
            durability: current_deps.durability,
            // lifetime erase for storage
            fields: unsafe { mem::transmute::<C::Fields<'db>, C::Fields<'static>>(fields) },
            revisions: C::new_revisions(current_deps.changed_at),
            // SAFETY: We only ever access the memos of a value that we allocated through
            // our `MemoTableTypes`.
            memos: unsafe { MemoTable::new(self.memo_table_types()) },
        };

        while let Some(id) = self.free_list.pop() {
            // Increment the ID generation before reusing it, as if we have allocated a new
            // slot in the table.
            //
            // If the generation would overflow, we are forced to leak the slot. Note that this
            // shouldn't be a problem in general as sufficient bits are reserved for the generation.
            let Some(id) = id.next_generation() else {
                crate::tracing::info!(
                    "leaking tracked struct {:?} due to generation overflow",
                    self.database_key_index(id)
                );

                continue;
            };

            // SAFETY: We just removed `id` from the free-list, so we have exclusive access.
            let data = unsafe { &mut *Self::data_raw(zalsa.table(), id) };

            assert!(
                data.updated_at.load().is_none(),
                "free list entry for `{id:?}` does not have `None` for `updated_at`"
            );

            // Overwrite the free-list entry. Use `*foo = ` because the entry
            // has been previously initialized and we want to free the old contents.
            *data = value(id);

            return id;
        }

        let (id, _) = zalsa_local.allocate::<Value<C>>(zalsa, self.ingredient_index, value);

        id
    }

    /// Get mutable access to the data for `id` -- this holds a write lock for the duration
    /// of the returned value.
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value is already updated in this revision.
    ///
    /// # Safety
    ///
    /// The value at the given `id` must be initialized.
    unsafe fn update<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        current_revision: Revision,
        mut id: Id,
        current_deps: &Stamp,
        fields: C::Fields<'db>,
    ) -> Result<Id, C::Fields<'db>> {
        let data_raw = Self::data_raw(zalsa.table(), id);

        // The protocol is:
        //
        // * When we begin updating, we store `None` in the `updated_at` field
        // * When completed, we store `Some(current_revision)` in `updated_at`
        //
        // No matter what mischief users get up to, it should be impossible for us to
        // observe `None` in `updated_at`. The `id` should only be associated with one
        // query and that query can only be running in one thread at a time.
        //
        // We *can* observe `Some(current_revision)` however, which means that this
        // tracked struct is already updated for this revision in two ways.
        // In that case we should not modify or touch it because there may be
        // `&`-references to its contents floating around.
        //
        // Observing `Some(current_revision)` can happen in two scenarios: leaks (tsk tsk)
        // but also the scenario embodied by the test test `test_run_5_then_20` in `specify_tracked_fn_in_rev_1_but_not_2.rs`:
        //
        // * Revision 1:
        //   * Tracked function F creates tracked struct S
        //   * F reads input I
        // * Revision 2: I is changed, F is re-executed
        //
        // When F is re-executed in rev 2, we first try to validate F's inputs/outputs,
        // which is the list [output: S, input: I]. As no inputs have changed by the time
        // we reach S, we mark it as verified. But then input I is seen to have changed,
        // and so we re-execute F. Note that we *know* that S will have the same value
        // (barring program bugs).
        //
        // Further complicating things: it is possible that F calls F2
        // and gives it (e.g.) S as one of its arguments. Validating F2 may cause F2 to
        // re-execute which means that it may indeed have read from S's fields
        // during the current revision and thus obtained an `&` reference to those fields
        // that is still live.

        {
            // SAFETY: Guaranteed by caller.
            let data = unsafe { &*data_raw };

            let last_updated_at = data.updated_at.load();
            assert!(
                last_updated_at.is_some(),
                "two concurrent writers to {id:?}, should not be possible"
            );

            // The value is already read-locked, but we can reuse it safely as per above.
            if last_updated_at == Some(current_revision) {
                return Ok(id);
            }

            // Updating the fields may make it necessary to increment the generation of the ID. In
            // the unlikely case that the ID is already at its maximum generation, we are forced to leak
            // the previous slot and allocate a new value.
            if id.generation() == u32::MAX {
                crate::tracing::info!(
                    "leaking tracked struct {:?} due to generation overflow",
                    self.database_key_index(id)
                );

                return Err(fields);
            }

            // Acquire the write-lock. This can only fail if there is a parallel thread
            // reading from this same `id`, which can only happen if the user has leaked it.
            // Tsk tsk.
            let swapped_out = data.updated_at.swap(None);
            if swapped_out != last_updated_at {
                panic!(
                "failed to acquire write lock, id `{id:?}` must have been leaked across threads"
            );
            }
        }

        // UNSAFE: Marking as mut requires exclusive access for the duration of
        // the `mut`. We have now *claimed* this data by swapping in `None`,
        // any attempt to read concurrently will panic.
        let data = unsafe { &mut *data_raw };

        // SAFETY: We assert that the pointer to `data.revisions`
        // is a pointer into the database referencing a value
        // from a previous revision. As such, it continues to meet
        // its validity invariant and any owned content also continues
        // to meet its safety invariant.
        let untracked_update = unsafe {
            C::update_fields(
                current_deps.changed_at,
                &mut data.revisions,
                mem::transmute::<*mut C::Fields<'static>, *mut C::Fields<'db>>(
                    std::ptr::addr_of_mut!(data.fields),
                ),
                fields,
            )
        };

        if untracked_update {
            // Consider this a new tracked-struct when any non-tracked field got updated.
            // This should be rare and only ever happen if there's a hash collision.
            //
            // Note that we hold the lock and have exclusive access to the tracked struct data,
            // so there should be no live instances of IDs from the previous generation. We clear
            // the memos and return a new ID here as if we have allocated a new slot.
            let memo_table = data.memo_table_mut();

            // SAFETY: The memo table belongs to a value that we allocated, so it has the
            // correct type.
            unsafe { self.clear_memos(zalsa, memo_table, id) };

            id = id
                .next_generation()
                .expect("already verified that generation is not maximum");
        }

        if current_deps.durability < data.durability {
            data.revisions = C::new_revisions(current_deps.changed_at);
        }
        data.durability = current_deps.durability;
        let swapped_out = data.updated_at.swap(Some(current_revision));
        assert!(swapped_out.is_none());

        Ok(id)
    }

    /// Fetch the data for a given id created by this ingredient from the table,
    /// -giving it the appropriate type.
    fn data(table: &Table, id: Id) -> &Value<C> {
        table.get(id)
    }

    fn data_raw(table: &Table, id: Id) -> *mut Value<C> {
        table.get_raw(id)
    }

    /// Deletes the given entities. This is used after a query `Q` executes and we can compare
    /// the entities `E_now` that it produced in this revision vs the entities
    /// `E_prev` it produced in the last revision. Any missing entities `E_prev - E_new` can be
    /// deleted.
    ///
    /// # Warning
    ///
    /// Using this method on an entity id that MAY be used in the current revision will lead to
    /// unspecified results (but not UB). See [`InternedIngredient::delete_index`] for more
    /// discussion and important considerations.
    pub(crate) fn delete_entity(&self, zalsa: &Zalsa, id: Id) {
        zalsa.event(&|| {
            Event::new(crate::EventKind::DidDiscard {
                key: self.database_key_index(id),
            })
        });

        let current_revision = zalsa.current_revision();
        let data_raw = Self::data_raw(zalsa.table(), id);

        {
            let data = unsafe { &*data_raw };

            // We want to set `updated_at` to `None`, signalling that other field values
            // cannot be read. The current value should be `Some(R0)` for some older revision.
            match data.updated_at.load() {
                None => {
                    panic!("cannot delete write-locked id `{id:?}`; value leaked across threads");
                }
                Some(r) if r == current_revision => panic!(
                    "cannot delete read-locked id `{id:?}`; value leaked across threads or user functions not deterministic"
                ),
                Some(r) => {
                    if data.updated_at.compare_exchange(Some(r), None).is_err() {
                        panic!("race occurred when deleting value `{id:?}`")
                    }
                }
            }
        }

        // SAFETY: We have acquired the write lock
        let data = unsafe { &mut *data_raw };
        let memo_table = data.memo_table_mut();

        // SAFETY: The memo table belongs to a value that we allocated, so it
        // has the correct type.
        unsafe { self.clear_memos(zalsa, memo_table, id) };

        // now that all cleanup has occurred, make available for re-use
        self.free_list.push(id);
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

        mem::forget(table_guard);

        // Reset the table after having dropped any memos.
        memo_table.reset();
    }

    /// Return reference to the field data ignoring dependency tracking.
    /// Used for debugging.
    pub fn leak_fields<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        s: C::Struct<'db>,
    ) -> &'db C::Fields<'db> {
        let id = AsId::as_id(&s);
        let data = Self::data(zalsa.table(), id);
        data.fields()
    }

    /// Access to this tracked field.
    ///
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responsible for selecting the appropriate element.
    pub fn tracked_field<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        s: C::Struct<'db>,
        relative_tracked_index: usize,
    ) -> &'db C::Fields<'db> {
        let id = AsId::as_id(&s);
        let field_ingredient_index = self.ingredient_index.successor(relative_tracked_index);
        let data = Self::data(zalsa.table(), id);

        data.read_lock(zalsa.current_revision());

        let field_changed_at = data.revisions[relative_tracked_index];

        zalsa_local.report_tracked_read_simple(
            DatabaseKeyIndex::new(field_ingredient_index, id),
            data.durability,
            field_changed_at,
        );

        data.fields()
    }

    /// Access to this untracked field.
    ///
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responsible for selecting the appropriate element.
    pub fn untracked_field<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        s: C::Struct<'db>,
    ) -> &'db C::Fields<'db> {
        let id = AsId::as_id(&s);
        let data = Self::data(zalsa.table(), id);

        data.read_lock(zalsa.current_revision());

        // Note that we do not need to add a dependency on the tracked struct
        // as IDs that are reused increment their generation, invalidating any
        // dependent queries directly.

        data.fields()
    }

    /// Returns all data corresponding to the tracked struct.
    pub fn entries<'db>(&'db self, zalsa: &'db Zalsa) -> impl Iterator<Item = StructEntry<'db, C>> {
        zalsa
            .table()
            .slots_of::<Value<C>>()
            .map(|(id, value)| StructEntry {
                value,
                key: self.database_key_index(id),
            })
    }
}

/// A tracked struct entry.
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

    /// Returns the tracked struct.
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
        _zalsa: &crate::zalsa::Zalsa,
        _db: crate::database::RawDatabase<'_>,
        _input: Id,
        _revision: Revision,
        _cycle_heads: &mut VerifyCycleHeads,
    ) -> VerifyResult {
        // Any change to a tracked struct results in a new ID generation, so there
        // are no direct dependencies on the struct, only on its tracked fields.
        panic!("nothing should ever depend on a tracked struct directly")
    }

    fn collect_minimum_serialized_edges(
        &self,
        _zalsa: &Zalsa,
        _edge: QueryEdge,
        _serialized_edges: &mut FxIndexSet<QueryEdge>,
        _visited_edges: &mut FxHashSet<QueryEdge>,
    ) {
        // Note that tracked structs are referenced by the identity map, but that
        // only matters if we are serializing the creating query, in which case
        // the dependency edge will be serialized directly.
        //
        // TODO: We could flatten the identity map here if the tracked struct is being
        // persisted, in order to more aggressively preserve the tracked struct IDs if
        // the transitive query is re-executed.
        panic!("nothing should ever depend on a tracked struct directly")
    }

    fn remove_stale_output(
        &self,
        zalsa: &Zalsa,
        _executor: DatabaseKeyIndex,
        stale_output_key: crate::Id,
    ) {
        // This method is called when, in prior revisions,
        // `executor` creates a tracked struct `salsa_output_key`,
        // but it did not in the current revision.
        // In that case, we can delete `stale_output_key` and any data associated with it.
        self.delete_entity(zalsa, stale_output_key)
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

    /// Returns memory usage information about any tracked structs.
    #[cfg(feature = "salsa_unstable")]
    fn memory_usage(&self, db: &dyn crate::Database) -> Option<Vec<crate::database::SlotInfo>> {
        let memory_usage = self
            .entries(db.zalsa())
            // SAFETY: The memo table belongs to a value that we allocated, so it
            // has the correct type.
            .map(|entry| unsafe { entry.value.memory_usage(&self.memo_table_types) })
            .collect();

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
            _ingredient: self,
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
            .field("ingredient_index", &self.ingredient_index)
            .finish()
    }
}

impl<C> Value<C>
where
    C: Configuration,
{
    /// Fields of this tracked struct.
    ///
    /// They can change across revisions, but they do not change within
    /// a particular revision.
    #[cfg_attr(not(feature = "salsa_unstable"), doc(hidden))]
    pub fn fields(&self) -> &C::Fields<'_> {
        // SAFETY: We are shrinking the lifetime from storage back to the db lifetime.
        unsafe { mem::transmute::<&C::Fields<'static>, &C::Fields<'_>>(&self.fields) }
    }

    fn memo_table_mut(&mut self) -> &mut MemoTable {
        // This fn is only called after `updated_at` has been set to `None`;
        // this ensures that there is no concurrent access
        // (and that the `&mut self` is accurate...).
        assert!(self.updated_at.load().is_none());
        &mut self.memos
    }

    fn read_lock(&self, current_revision: Revision) {
        loop {
            match self.updated_at.load() {
                None => {
                    panic!("access to field whilst the value is being initialized");
                }
                Some(r) => {
                    if r == current_revision {
                        return;
                    }

                    if self
                        .updated_at
                        .compare_exchange(Some(r), Some(current_revision))
                        .is_ok()
                    {
                        break;
                    }
                }
            }
        }
    }

    /// Returns memory usage information about the tracked struct.
    ///
    /// # Safety
    ///
    /// The `MemoTable` must belong to a `Value` of the correct type.
    #[cfg(feature = "salsa_unstable")]
    unsafe fn memory_usage(&self, memo_table_types: &MemoTableTypes) -> crate::database::SlotInfo {
        let heap_size = C::heap_size(self.fields());
        // SAFETY: The caller guarantees this is the correct types table.
        let memos = unsafe { memo_table_types.attach_memos(&self.memos) };

        crate::database::SlotInfo {
            debug_name: C::DEBUG_NAME,
            size_of_metadata: mem::size_of::<Self>() - mem::size_of::<C::Fields<'_>>(),
            size_of_fields: mem::size_of::<C::Fields<'_>>(),
            heap_size_of_fields: heap_size,
            memos: memos.memory_usage(),
        }
    }
}

// SAFETY: `Value<C>` is our private type branded over the unique configuration `C`.
unsafe impl<C> Slot for Value<C>
where
    C: Configuration,
{
    #[inline(always)]
    unsafe fn memos(&self, current_revision: Revision) -> &crate::table::memo::MemoTable {
        // Acquiring the read lock here with the current revision to ensure that there
        // is no danger of a race when deleting a tracked struct.
        self.read_lock(current_revision);
        &self.memos
    }

    #[inline(always)]
    fn memos_mut(&mut self) -> &mut crate::table::memo::MemoTable {
        &mut self.memos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disambiguate_map_works() {
        let mut d = DisambiguatorMap::default();
        // set up all 4 permutations of differing field values
        let h1 = IdentityHash {
            ingredient_index: IngredientIndex::new(0),
            hash: 0,
        };
        let h2 = IdentityHash {
            ingredient_index: IngredientIndex::new(1),
            hash: 0,
        };
        let h3 = IdentityHash {
            ingredient_index: IngredientIndex::new(0),
            hash: 1,
        };
        let h4 = IdentityHash {
            ingredient_index: IngredientIndex::new(1),
            hash: 1,
        };
        assert_eq!(d.disambiguate(h1), Disambiguator(0));
        assert_eq!(d.disambiguate(h1), Disambiguator(1));
        assert_eq!(d.disambiguate(h2), Disambiguator(0));
        assert_eq!(d.disambiguate(h2), Disambiguator(1));
        assert_eq!(d.disambiguate(h3), Disambiguator(0));
        assert_eq!(d.disambiguate(h3), Disambiguator(1));
        assert_eq!(d.disambiguate(h4), Disambiguator(0));
        assert_eq!(d.disambiguate(h4), Disambiguator(1));
    }

    #[test]
    fn identity_map_works() {
        let mut d = IdentityMap::default();
        // set up all 8 permutations of differing field values
        let i1 = Identity {
            ingredient_index: IngredientIndex::new(0),
            hash: 0,
            disambiguator: Disambiguator(0),
        };
        let i2 = Identity {
            ingredient_index: IngredientIndex::new(1),
            hash: 0,
            disambiguator: Disambiguator(0),
        };
        let i3 = Identity {
            ingredient_index: IngredientIndex::new(0),
            hash: 1,
            disambiguator: Disambiguator(0),
        };
        let i4 = Identity {
            ingredient_index: IngredientIndex::new(1),
            hash: 1,
            disambiguator: Disambiguator(0),
        };
        let i5 = Identity {
            ingredient_index: IngredientIndex::new(0),
            hash: 0,
            disambiguator: Disambiguator(1),
        };
        let i6 = Identity {
            ingredient_index: IngredientIndex::new(1),
            hash: 0,
            disambiguator: Disambiguator(1),
        };
        let i7 = Identity {
            ingredient_index: IngredientIndex::new(0),
            hash: 1,
            disambiguator: Disambiguator(1),
        };
        let i8 = Identity {
            ingredient_index: IngredientIndex::new(1),
            hash: 1,
            disambiguator: Disambiguator(1),
        };
        // SAFETY: We don't use the IDs within salsa internals so this is fine
        unsafe {
            assert_eq!(d.insert(i1, Id::from_index(0)), None);
            assert_eq!(d.insert(i2, Id::from_index(1)), None);
            assert_eq!(d.insert(i3, Id::from_index(2)), None);
            assert_eq!(d.insert(i4, Id::from_index(3)), None);
            assert_eq!(d.insert(i5, Id::from_index(4)), None);
            assert_eq!(d.insert(i6, Id::from_index(5)), None);
            assert_eq!(d.insert(i7, Id::from_index(6)), None);
            assert_eq!(d.insert(i8, Id::from_index(7)), None);

            assert_eq!(d.reuse(&i1), Some(Id::from_index(0)));
            assert_eq!(d.reuse(&i2), Some(Id::from_index(1)));
            assert_eq!(d.reuse(&i3), Some(Id::from_index(2)));
            assert_eq!(d.reuse(&i4), Some(Id::from_index(3)));
            assert_eq!(d.reuse(&i5), Some(Id::from_index(4)));
            assert_eq!(d.reuse(&i6), Some(Id::from_index(5)));
            assert_eq!(d.reuse(&i7), Some(Id::from_index(6)));
            assert_eq!(d.reuse(&i8), Some(Id::from_index(7)));
        };
    }
}

#[cfg(feature = "persistence")]
mod persistence {
    use std::fmt;

    use serde::ser::{SerializeMap, SerializeStruct};
    use serde::{de, Deserialize};

    use super::{Configuration, IngredientImpl, Value};
    use crate::plumbing::Ingredient;
    use crate::revision::OptionalAtomicRevision;
    use crate::table::memo::MemoTable;
    use crate::zalsa::Zalsa;
    use crate::{Durability, Id};

    pub struct SerializeIngredient<'db, C>
    where
        C: Configuration,
    {
        pub zalsa: &'db Zalsa,
        pub _ingredient: &'db IngredientImpl<C>,
    }

    impl<C> serde::Serialize for SerializeIngredient<'_, C>
    where
        C: Configuration,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let Self { zalsa, .. } = self;

            let count = zalsa.table().slots_of::<Value<C>>().count();
            let mut map = serializer.serialize_map(Some(count))?;

            for (id, value) in zalsa.table().slots_of::<Value<C>>() {
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
            let mut value = serializer.serialize_struct("Value", 4)?;

            let Value {
                durability,
                updated_at,
                fields,
                revisions,
                memos: _,
            } = self;

            value.serialize_field("durability", &durability)?;
            value.serialize_field("updated_at", &updated_at)?;
            value.serialize_field("revisions", &revisions)?;
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

                let value = Value::<C> {
                    updated_at: value.updated_at,
                    durability: value.durability,
                    fields: value.fields.0,
                    revisions: value.revisions,
                    // SAFETY: We only ever access the memos of a value that we allocated through
                    // our `MemoTableTypes`.
                    memos: unsafe { MemoTable::new(ingredient.memo_table_types()) },
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
                let (allocated_id, _) = unsafe {
                    zalsa
                        .table()
                        .page(page_idx)
                        .allocate(page_idx, |_| value)
                        .unwrap_or_else(|_| panic!("serialized an invalid `Id`: {id:?}"))
                };

                assert_eq!(
                    allocated_id, id,
                    "values are serialized in allocation order"
                );
            }

            Ok(())
        }
    }

    #[derive(Deserialize)]
    #[serde(rename = "Value")]
    pub struct DeserializeValue<C: Configuration> {
        durability: Durability,
        updated_at: OptionalAtomicRevision,
        revisions: C::Revisions,
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
