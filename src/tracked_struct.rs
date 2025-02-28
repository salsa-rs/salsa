use std::{any::TypeId, fmt, hash::Hash, marker::PhantomData, ops::DerefMut};

use crossbeam_queue::SegQueue;
use tracked_field::FieldIngredientImpl;

use crate::{
    accumulator::accumulated_map::InputAccumulatedValues,
    cycle::CycleRecoveryStrategy,
    ingredient::{fmt_index, Ingredient, Jar, MaybeChangedAfter},
    key::{DatabaseKeyIndex, InputDependencyIndex},
    plumbing::ZalsaLocal,
    revision::OptionalAtomicRevision,
    runtime::StampedValue,
    salsa_struct::SalsaStructInDb,
    table::{memo::MemoTable, sync::SyncTable, Slot, Table},
    zalsa::{IngredientIndex, Zalsa},
    zalsa_local::QueryOrigin,
    Database, Durability, Event, EventKind, Id, Revision,
};

pub mod tracked_field;

// ANCHOR: Configuration
/// Trait that defines the key properties of a tracked struct.
/// Implemented by the `#[salsa::tracked]` macro when applied
/// to a struct.
pub trait Configuration: Sized + 'static {
    /// The debug name of the tracked struct.
    const DEBUG_NAME: &'static str;

    /// The debug names of any fields.
    const FIELD_DEBUG_NAMES: &'static [&'static str];

    /// The relative indices of any tracked fields.
    const TRACKED_FIELD_INDICES: &'static [usize];

    /// A (possibly empty) tuple of the fields for this struct.
    type Fields<'db>: Send + Sync;

    /// A array of [`Revision`][] values, one per each of the tracked value fields.
    /// When a struct is re-recreated in a new revision, the corresponding
    /// entries for each field are updated to the new revision if their
    /// values have changed (or if the field is marked as `#[no_eq]`).
    type Revisions: Send + Sync + DerefMut<Target = [Revision]>;

    type Struct<'db>: Copy;

    /// Create an end-user struct from the underlying raw pointer.
    ///
    /// This call is an "end-step" to the tracked struct lookup/creation
    /// process in a given revision: it occurs only when the struct is newly
    /// created or, if a struct is being reused, after we have updated its
    /// fields (or confirmed it is green and no updates are required).
    fn struct_from_id<'db>(id: Id) -> Self::Struct<'db>;

    /// Deref the struct to yield the underlying id.
    fn deref_struct(s: Self::Struct<'_>) -> Id;

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
        _zalsa: &Zalsa,
        struct_index: crate::zalsa::IngredientIndex,
        _dependencies: crate::memo_ingredient_indices::IngredientIndices,
    ) -> Vec<Box<dyn Ingredient>> {
        let struct_ingredient = <IngredientImpl<C>>::new(struct_index);

        let tracked_field_ingredients =
            C::TRACKED_FIELD_INDICES
                .iter()
                .copied()
                .map(|relative_tracked_index| {
                    Box::new(<FieldIngredientImpl<C>>::new(
                        relative_tracked_index,
                        struct_index.successor(relative_tracked_index),
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
    fn database_key_index(db: &dyn Database, id: Id) -> DatabaseKeyIndex;
}

/// Created for each tracked struct.
///
/// This ingredient only stores the "id" fields. It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked
/// struct id. The value fields are stored in [`crate::function::FunctionIngredient`]
/// instances keyed by the tracked struct id. Unlike normal interners, tracked
/// struct indices can be deleted and reused aggressively: when a tracked
/// function re-executes, any tracked structs that it created before but did
/// not create this time can be deleted.
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
}

/// Defines the identity of a tracked struct.
/// This is the key to a hashmap that is (initially)
/// stored in the [`ActiveQuery`](`crate::active_query::ActiveQuery`)
/// struct and later moved to the [`Memo`](`crate::function::memo::Memo`).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
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
/// This includes the ingredient index of that struct type plus the hash of its untracked fields.
/// This is mapped to a disambiguator -- a value that starts as 0 but increments each round,
/// allowing for multiple tracked structs with the same hash and ingredient_index
/// created within the query to each have a unique id.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub struct IdentityHash {
    /// Index of the tracked struct ingredient.
    ingredient_index: IngredientIndex,

    /// Hash of the id fields.
    hash: u64,
}

#[derive(Default, Debug)]
pub(crate) struct IdentityMap {
    // we use a non-hasher hashmap here as our key contains its own hash (`Identity::hash`)
    // so we use the raw entry api instead to avoid the overhead of hashing unnecessarily
    map: hashbrown::HashMap<Identity, Id, ()>,
}

impl Clone for IdentityMap {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
    fn clone_from(&mut self, source: &Self) {
        self.map.clone_from(&source.map);
    }
}

impl IdentityMap {
    pub(crate) fn insert(&mut self, key: Identity, id: Id) -> Option<Id> {
        use hashbrown::hash_map::RawEntryMut;

        let entry = self.map.raw_entry_mut().from_hash(key.hash, |k| *k == key);
        match entry {
            RawEntryMut::Occupied(mut occupied) => Some(occupied.insert(id)),
            RawEntryMut::Vacant(vacant) => {
                vacant.insert_with_hasher(key.hash, key, id, |k| k.hash);
                None
            }
        }
    }

    pub(crate) fn get(&self, key: &Identity) -> Option<Id> {
        self.map
            .raw_entry()
            .from_hash(key.hash, |k| *k == *key)
            .map(|(_, &v)| v)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub(crate) fn retain(&mut self, f: impl FnMut(&Identity, &mut Id) -> bool) {
        self.map.retain(f);
    }
}

// ANCHOR: ValueStruct
#[derive(Debug)]
pub struct Value<C>
where
    C: Configuration,
{
    /// The durability minimum durability of all inputs consumed
    /// by the creator query prior to creating this tracked struct.
    /// If any of those inputs changes, then the creator query may
    /// create this struct with different values.
    durability: Durability,

    /// The revision in which the tracked struct was first created.
    ///
    /// Unlike `updated_at`, which gets bumped on every read,
    /// `created_at` is updated whenever an untracked field is updated.
    /// This is necessary to detect reused tracked struct ids _after_
    /// they've been freed in a prior revision or tracked structs that have been updated
    /// in-place because of a bad `Hash` implementation.
    created_at: Revision,

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
    memos: MemoTable,

    /// Sync table storing the results of query functions etc.
    syncs: SyncTable,
}
// ANCHOR_END: ValueStruct

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
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
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Convert the fields from a `'db` lifetime to `'static`: used when storing
    /// the data into this ingredient, should never be released outside this type.
    unsafe fn to_static<'db>(&'db self, fields: C::Fields<'db>) -> C::Fields<'static> {
        unsafe { std::mem::transmute(fields) }
    }

    unsafe fn to_self_ref<'db>(&'db self, fields: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
        unsafe { std::mem::transmute(fields) }
    }

    /// Convert from static back to the db lifetime; used when returning data
    /// out from this ingredient.
    unsafe fn to_self_ptr<'db>(&'db self, fields: *mut C::Fields<'static>) -> *mut C::Fields<'db> {
        unsafe { std::mem::transmute(fields) }
    }

    /// Create a tracked struct ingredient. Generated by the `#[tracked]` macro,
    /// not meant to be called directly by end-users.
    fn new(index: IngredientIndex) -> Self {
        Self {
            ingredient_index: index,
            phantom: PhantomData,
            free_list: Default::default(),
        }
    }

    /// Returns the database key index for a tracked struct with the given id.
    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.ingredient_index,
            key_index: id,
        }
    }

    pub fn new_struct<'db>(
        &'db self,
        db: &'db dyn Database,
        fields: C::Fields<'db>,
    ) -> C::Struct<'db> {
        let (zalsa, zalsa_local) = db.zalsas();

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

        match zalsa_local.tracked_struct_id(&identity) {
            Some(id) => {
                // The struct already exists in the intern map.
                zalsa_local.add_output(self.database_key_index(id).into());
                self.update(zalsa, id, &current_deps, fields);
                C::struct_from_id(id)
            }

            None => {
                // This is a new tracked struct, so create an entry in the struct map.
                let id = self.allocate(zalsa, zalsa_local, &current_deps, fields);
                let key = self.database_key_index(id);
                zalsa_local.add_output(key.into());
                zalsa_local.store_tracked_struct_id(identity, id);
                C::struct_from_id(id)
            }
        }
    }

    fn allocate<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        zalsa_local: &'db ZalsaLocal,
        current_deps: &StampedValue<()>,
        fields: C::Fields<'db>,
    ) -> Id {
        let current_revision = zalsa.current_revision();
        let value = |_| Value {
            created_at: current_revision,
            updated_at: OptionalAtomicRevision::new(Some(current_revision)),
            durability: current_deps.durability,
            fields: unsafe { self.to_static(fields) },
            revisions: C::new_revisions(current_deps.changed_at),
            memos: Default::default(),
            syncs: Default::default(),
        };

        if let Some(id) = self.free_list.pop() {
            let data_raw = Self::data_raw(zalsa.table(), id);
            debug_assert!(
                unsafe { (*data_raw).updated_at.load().is_none() },
                "free list entry for `{id:?}` should not be locked"
            );

            // Overwrite the free-list entry. Use `*foo = ` because the entry
            // has been previously initialized and we want to free the old contents.
            unsafe { *data_raw = value(id) };

            id
        } else {
            zalsa_local.allocate::<Value<C>>(zalsa.table(), self.ingredient_index, value)
        }
    }

    /// Get mutable access to the data for `id` -- this holds a write lock for the duration
    /// of the returned value.
    ///
    /// # Panics
    ///
    /// * If the value is not present in the map.
    /// * If the value is already updated in this revision.
    fn update<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        current_deps: &StampedValue<()>,
        fields: C::Fields<'db>,
    ) {
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

        let current_revision = zalsa.current_revision();
        // UNSAFE: Marking as mut requires exclusive access for the duration of
        // the `mut`. We have now *claimed* this data by swapping in `None`,
        // any attempt to read concurrently will panic.
        let last_updated_at = unsafe { (*data_raw).updated_at.load() };
        assert!(
            last_updated_at.is_some(),
            "two concurrent writers to {id:?}, should not be possible"
        );
        if last_updated_at == Some(current_revision) {
            // already read-locked
            return;
        }

        // Acquire the write-lock. This can only fail if there is a parallel thread
        // reading from this same `id`, which can only happen if the user has leaked it.
        // Tsk tsk.

        let swapped = unsafe { (*data_raw).updated_at.swap(None) };
        if last_updated_at != swapped {
            panic!(
                "failed to acquire write lock, id `{id:?}` must have been leaked across threads"
            );
        }

        // SAFETY: Marking as mut requires exclusive access for the duration of
        // the `mut`. We have now *claimed* this data by swapping in `None`,
        // any attempt to read concurrently will panic. Note that we cannot create
        // a `&mut` reference to the full `Value` though because
        // another thread may access `updated_at` concurrently.

        // SAFETY: We assert that the pointer to `data.revisions`
        // is a pointer into the database referencing a value
        // from a previous revision. As such, it continues to meet
        // its validity invariant and any owned content also continues
        // to meet its safety invariant.
        unsafe {
            if C::update_fields(
                current_revision,
                &mut (*data_raw).revisions,
                self.to_self_ptr(std::ptr::addr_of_mut!((*data_raw).fields)),
                fields,
            ) {
                // Consider this a new tracked-struct (even though it still uses the same id)
                // when any non-tracked field got updated.
                // This should be rare and only ever happen if there's a hash collision
                // which makes Salsa consider two tracked structs to still be the same
                // even though the fields are different.
                // See `tracked-struct-id-field-bad-hash` for more details.
                (*data_raw).revisions = C::new_revisions(current_revision);
                (*data_raw).created_at = current_revision;
            } else if current_deps.durability < (*data_raw).durability {
                (*data_raw).revisions = C::new_revisions(current_revision);
                (*data_raw).created_at = current_revision;
            }
            (*data_raw).durability = current_deps.durability;
        }
        let swapped_out = unsafe { (*data_raw).updated_at.swap_mut(Some(current_revision)) };
        assert!(swapped_out.is_none(), "lock was acquired twice!");
    }

    /// Fetch the data for a given id created by this ingredient from the table,
    /// -giving it the appropriate type.
    fn data(table: &Table, id: Id, current_revision: Revision) -> &Value<C> {
        let val = Self::data_raw(table, id);
        acquire_read_lock(unsafe { &(*val).updated_at }, current_revision);
        // We have acquired the read lock, so it is safe to return a reference to the data.
        unsafe { &*val }
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
    pub(crate) fn delete_entity(&self, db: &dyn crate::Database, id: Id) {
        db.salsa_event(&|| {
            Event::new(crate::EventKind::DidDiscard {
                key: self.database_key_index(id),
            })
        });

        let zalsa = db.zalsa();
        let data = Self::data_raw(zalsa.table(), id);

        // We want to set `updated_at` to `None`, signalling that other field values
        // cannot be read. The current value should be `Some(R0)` for some older revision.
        match unsafe { (*data).updated_at.swap(None) }{
            None => {
                panic!("cannot delete write-locked id `{id:?}`; value leaked across threads");
            }
            Some(r) if r == zalsa.current_revision() => panic!(
                "cannot delete read-locked id `{id:?}`; value leaked across threads or user functions not deterministic"
            ),
            Some(_) => ()
        }

        // Take the memo table. This is safe because we have modified `data_ref.updated_at` to `None`
        // signalling that we have acquired the write lock
        let memo_table = std::mem::take(unsafe { &mut (*data).memos });

        // SAFETY: We have verified that no more references to these memos exist and so we are good
        // to drop them.
        for (memo_ingredient_index, memo) in unsafe { memo_table.into_memos() } {
            let ingredient_index =
                zalsa.ingredient_index_for_memo(self.ingredient_index, memo_ingredient_index);

            let executor = DatabaseKeyIndex {
                ingredient_index,
                key_index: id,
            };

            db.salsa_event(&|| Event::new(EventKind::DidDiscard { key: executor }));

            for stale_output in memo.origin().outputs() {
                stale_output.remove_stale_output(zalsa, db, executor);
            }
        }

        // now that all cleanup has occurred, make available for re-use
        self.free_list.push(id);
    }

    /// Return reference to the field data ignoring dependency tracking.
    /// Used for debugging.
    pub fn leak_fields<'db>(
        &'db self,
        db: &'db dyn Database,
        s: C::Struct<'db>,
    ) -> &'db C::Fields<'db> {
        let id = C::deref_struct(s);
        let value = Self::data(db.zalsa().table(), id, db.zalsa().current_revision());
        unsafe { self.to_self_ref(&value.fields) }
    }

    /// Access to this tracked field.
    ///
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responsible for selecting the appropriate element.
    ///
    /// This function takes two indices:
    /// - `field_index` is the absolute index of the field on the tracked struct.
    /// - `relative_tracked_index` is the index of the field relative only to other
    ///   tracked fields.
    pub fn tracked_field<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        s: C::Struct<'db>,
        relative_tracked_index: usize,
    ) -> &'db C::Fields<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        let id = C::deref_struct(s);
        let field_ingredient_index = self.ingredient_index.successor(relative_tracked_index);
        let data = Self::data(zalsa.table(), id, zalsa.current_revision());

        let field_changed_at = data.revisions[relative_tracked_index];

        zalsa_local.report_tracked_read(
            InputDependencyIndex::new(field_ingredient_index, id),
            data.durability,
            field_changed_at,
            InputAccumulatedValues::Empty,
        );

        unsafe { self.to_self_ref(&data.fields) }
    }

    /// Access to this untracked field.
    ///
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responsible for selecting the appropriate element.
    pub fn untracked_field<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        s: C::Struct<'db>,
    ) -> &'db C::Fields<'db> {
        let (zalsa, zalsa_local) = db.zalsas();
        let id = C::deref_struct(s);
        let data = Self::data(zalsa.table(), id, zalsa.current_revision());

        // Add a dependency on the tracked struct itself.
        zalsa_local.report_tracked_read(
            InputDependencyIndex::new(self.ingredient_index, id),
            data.durability,
            data.created_at,
            InputAccumulatedValues::Empty,
        );

        unsafe { self.to_self_ref(&data.fields) }
    }

    #[cfg(feature = "salsa_unstable")]
    /// Returns all data corresponding to the tracked struct.
    pub fn entries<'db>(
        &'db self,
        db: &'db dyn crate::Database,
    ) -> impl Iterator<Item = &'db Value<C>> {
        db.zalsa()
            .table()
            .pages
            .iter()
            .filter_map(|(_, page)| page.cast_type::<crate::table::Page<Value<C>>>())
            .flat_map(|page| page.slots())
    }
}

impl<C> Ingredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn maybe_changed_after(
        &self,
        db: &dyn Database,
        input: Id,
        revision: Revision,
    ) -> MaybeChangedAfter {
        let zalsa = db.zalsa();
        let data = Self::data(zalsa.table(), input, zalsa.current_revision());

        MaybeChangedAfter::from(data.created_at > revision)
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        crate::cycle::CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _db: &dyn Database, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output<'db>(
        &'db self,
        _db: &'db dyn Database,
        _executor: DatabaseKeyIndex,
        _output_key: crate::Id,
    ) {
        // we used to update `update_at` field but now we do it lazily when data is accessed
    }

    fn remove_stale_output(
        &self,
        db: &dyn Database,
        _executor: DatabaseKeyIndex,
        stale_output_key: crate::Id,
    ) {
        // This method is called when, in prior revisions,
        // `executor` creates a tracked struct `salsa_output_key`,
        // but it did not in the current revision.
        // In that case, we can delete `stale_output_key` and any data associated with it.
        self.delete_entity(db, stale_output_key);
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(C::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
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
    #[cfg(feature = "salsa_unstable")]
    pub fn fields(&self) -> &C::Fields<'static> {
        &self.fields
    }
}

fn acquire_read_lock(updated_at: &OptionalAtomicRevision, current_revision: Revision) {
    loop {
        match updated_at.load() {
            None => panic!(
                "write lock taken; value leaked across threads or user functions not deterministic"
            ),
            // the read lock was taken by someone else, so we also succeed
            Some(r) if r == current_revision => return,
            Some(r) => {
                if updated_at
                    .compare_exchange(Some(r), Some(current_revision))
                    .is_ok()
                {
                    break;
                }
            }
        }
    }
}

impl<C> Slot for Value<C>
where
    C: Configuration,
{
    // FIXME: `&self` may alias here before the lock is taken?
    unsafe fn memos(&self, current_revision: Revision) -> &crate::table::memo::MemoTable {
        // Acquiring the read lock here with the current revision
        // ensures that there is no danger of a race
        // when deleting a tracked struct.
        acquire_read_lock(&self.updated_at, current_revision);
        &self.memos
    }

    fn memos_mut(&mut self) -> &mut crate::table::memo::MemoTable {
        &mut self.memos
    }

    // FIXME: `&self` may alias here?
    unsafe fn syncs(&self, current_revision: Revision) -> &crate::table::sync::SyncTable {
        // Acquiring the read lock here with the current revision
        // ensures that there is no danger of a race
        // when deleting a tracked struct.
        acquire_read_lock(&self.updated_at, current_revision);
        &self.syncs
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
            ingredient_index: IngredientIndex::from(0),
            hash: 0,
        };
        let h2 = IdentityHash {
            ingredient_index: IngredientIndex::from(1),
            hash: 0,
        };
        let h3 = IdentityHash {
            ingredient_index: IngredientIndex::from(0),
            hash: 1,
        };
        let h4 = IdentityHash {
            ingredient_index: IngredientIndex::from(1),
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
            ingredient_index: IngredientIndex::from(0),
            hash: 0,
            disambiguator: Disambiguator(0),
        };
        let i2 = Identity {
            ingredient_index: IngredientIndex::from(1),
            hash: 0,
            disambiguator: Disambiguator(0),
        };
        let i3 = Identity {
            ingredient_index: IngredientIndex::from(0),
            hash: 1,
            disambiguator: Disambiguator(0),
        };
        let i4 = Identity {
            ingredient_index: IngredientIndex::from(1),
            hash: 1,
            disambiguator: Disambiguator(0),
        };
        let i5 = Identity {
            ingredient_index: IngredientIndex::from(0),
            hash: 0,
            disambiguator: Disambiguator(1),
        };
        let i6 = Identity {
            ingredient_index: IngredientIndex::from(1),
            hash: 0,
            disambiguator: Disambiguator(1),
        };
        let i7 = Identity {
            ingredient_index: IngredientIndex::from(0),
            hash: 1,
            disambiguator: Disambiguator(1),
        };
        let i8 = Identity {
            ingredient_index: IngredientIndex::from(1),
            hash: 1,
            disambiguator: Disambiguator(1),
        };
        assert_eq!(d.insert(i1, Id::from_u32(0)), None);
        assert_eq!(d.insert(i2, Id::from_u32(1)), None);
        assert_eq!(d.insert(i3, Id::from_u32(2)), None);
        assert_eq!(d.insert(i4, Id::from_u32(3)), None);
        assert_eq!(d.insert(i5, Id::from_u32(4)), None);
        assert_eq!(d.insert(i6, Id::from_u32(5)), None);
        assert_eq!(d.insert(i7, Id::from_u32(6)), None);
        assert_eq!(d.insert(i8, Id::from_u32(7)), None);

        assert_eq!(d.get(&i1), Some(Id::from_u32(0)));
        assert_eq!(d.get(&i2), Some(Id::from_u32(1)));
        assert_eq!(d.get(&i3), Some(Id::from_u32(2)));
        assert_eq!(d.get(&i4), Some(Id::from_u32(3)));
        assert_eq!(d.get(&i5), Some(Id::from_u32(4)));
        assert_eq!(d.get(&i6), Some(Id::from_u32(5)));
        assert_eq!(d.get(&i7), Some(Id::from_u32(6)));
        assert_eq!(d.get(&i8), Some(Id::from_u32(7)));
    }
}
