use crossbeam::atomic::AtomicCell;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::alloc::Alloc;
use crate::durability::Durability;
use crate::id::{AsId, LookupId};
use crate::ingredient::{fmt_index, IngredientRequiresReset};
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryOrigin;
use crate::runtime::Runtime;
use crate::{DatabaseKeyIndex, Id};

use super::hash::FxDashMap;
use super::ingredient::Ingredient;
use super::routes::IngredientIndex;
use super::Revision;

pub trait Configuration: Sized + 'static {
    const DEBUG_NAME: &'static str;

    type Data<'db>: InternedData;

    type Struct<'db>: Copy;

    /// Create an end-user struct from the underlying raw pointer.
    ///
    /// This call is an "end-step" to the tracked struct lookup/creation
    /// process in a given revision: it occurs only when the struct is newly
    /// created or, if a struct is being reused, after we have updated its
    /// fields (or confirmed it is green and no updates are required).
    ///
    /// # Safety
    ///
    /// Requires that `ptr` represents a "confirmed" value in this revision,
    /// which means that it will remain valid and immutable for the remainder of this
    /// revision, represented by the lifetime `'db`.
    unsafe fn struct_from_raw<'db>(ptr: NonNull<ValueStruct<Self>>) -> Self::Struct<'db>;

    /// Deref the struct to yield the underlying value struct.
    /// Since we are still part of the `'db` lifetime in which the struct was created,
    /// this deref is safe, and the value-struct fields are immutable and verified.
    fn deref_struct(s: Self::Struct<'_>) -> &ValueStruct<Self>;
}

pub trait InternedData: Sized + Eq + Hash + Clone {}
impl<T: Eq + Hash + Clone> InternedData for T {}

/// The interned ingredient has the job of hashing values of type `Data` to produce an `Id`.
/// It used to store interned structs but also to store the id fields of a tracked struct.
/// Interned values endure until they are explicitly removed in some way.
pub struct InternedIngredient<C: Configuration> {
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,

    /// Maps from data to the existing interned id for that data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    key_map: FxDashMap<C::Data<'static>, Id>,

    /// Maps from an interned id to its data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    value_map: FxDashMap<Id, Alloc<ValueStruct<C>>>,

    /// counter for the next id.
    counter: AtomicCell<u32>,

    /// Stores the revision when this interned ingredient was last cleared.
    /// You can clear an interned table at any point, deleting all its entries,
    /// but that will make anything dependent on those entries dirty and in need
    /// of being recomputed.
    reset_at: Revision,
}

/// Struct storing the interned fields.
pub struct ValueStruct<C>
where
    C: Configuration,
{
    id: Id,
    fields: C::Data<'static>,
}

impl<C> InternedIngredient<C>
where
    C: Configuration,
{
    pub fn new(ingredient_index: IngredientIndex) -> Self {
        Self {
            ingredient_index,
            key_map: Default::default(),
            value_map: Default::default(),
            counter: AtomicCell::default(),
            reset_at: Revision::start(),
        }
    }

    unsafe fn to_internal_data<'db>(&'db self, data: C::Data<'db>) -> C::Data<'static> {
        unsafe { std::mem::transmute(data) }
    }

    pub fn intern_id<'db>(&'db self, runtime: &'db Runtime, data: C::Data<'db>) -> crate::Id {
        C::deref_struct(self.intern(runtime, data)).as_id()
    }

    /// Intern data to a unique reference.
    pub fn intern<'db>(&'db self, runtime: &'db Runtime, data: C::Data<'db>) -> C::Struct<'db> {
        runtime.report_tracked_read(
            DependencyIndex::for_table(self.ingredient_index),
            Durability::MAX,
            self.reset_at,
        );

        // Optimisation to only get read lock on the map if the data has already
        // been interned.
        let internal_data = unsafe { self.to_internal_data(data) };
        if let Some(guard) = self.key_map.get(&internal_data) {
            let id = *guard;
            drop(guard);
            return self.interned_value(id);
        }

        match self.key_map.entry(internal_data.clone()) {
            // Data has been interned by a racing call, use that ID instead
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                let id = *entry.get();
                drop(entry);
                self.interned_value(id)
            }

            // We won any races so should intern the data
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let next_id = self.counter.fetch_add(1);
                let next_id = crate::id::Id::from_u32(next_id);
                let value = self
                    .value_map
                    .entry(next_id)
                    .or_insert(Alloc::new(ValueStruct {
                        id: next_id,
                        fields: internal_data,
                    }));
                let value_raw = value.as_raw();
                drop(value);
                entry.insert(next_id);
                // SAFETY: Items are only removed from the `value_map` with an `&mut self` reference.
                unsafe { C::struct_from_raw(value_raw) }
            }
        }
    }

    pub fn interned_value(&self, id: Id) -> C::Struct<'_> {
        let r = self.value_map.get(&id).unwrap();

        // SAFETY: Items are only removed from the `value_map` with an `&mut self` reference.
        unsafe { C::struct_from_raw(r.as_raw()) }
    }

    /// Lookup the data for an interned value based on its id.
    /// Rarely used since end-users generally carry a struct with a pointer directly
    /// to the interned item.
    pub fn data(&self, id: Id) -> &C::Data<'_> {
        C::deref_struct(self.interned_value(id)).data()
    }

    /// Variant of `data` that takes a (unnecessary) database argument.
    /// This exists because tracked functions sometimes use true interning and sometimes use
    /// [`IdentityInterner`][], which requires the database argument.
    pub fn data_with_db<'db, DB: ?Sized>(&'db self, id: Id, _db: &'db DB) -> &'db C::Data<'db> {
        self.data(id)
    }

    pub fn reset(&mut self, revision: Revision) {
        assert!(revision > self.reset_at);
        self.reset_at = revision;
        self.key_map.clear();
        self.value_map.clear();
    }
}

impl<DB: ?Sized, C> Ingredient<DB> for InternedIngredient<C>
where
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn maybe_changed_after(&self, _db: &DB, _input: DependencyIndex, revision: Revision) -> bool {
        revision < self.reset_at
    }

    fn cycle_recovery_strategy(&self) -> crate::cycle::CycleRecoveryStrategy {
        crate::cycle::CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &DB,
        executor: DatabaseKeyIndex,
        output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "mark_validated_output({:?}, {:?}): input cannot be the output of a tracked function",
            executor, output_key
        );
    }

    fn remove_stale_output(
        &self,
        _db: &DB,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "remove_stale_output({:?}, {:?}): interned ids are not outputs",
            executor, stale_output_key
        );
    }

    fn reset_for_new_revision(&mut self) {
        // Interned ingredients do not, normally, get deleted except when they are "reset" en masse.
        // There ARE methods (e.g., `clear_deleted_entries` and `remove`) for deleting individual
        // items, but those are only used for tracked struct ingredients.
        panic!("unexpected call to `reset_for_new_revision`")
    }

    fn salsa_struct_deleted(&self, _db: &DB, _id: crate::Id) {
        panic!("unexpected call: interned ingredients do not register for salsa struct deletion events");
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(C::DEBUG_NAME, index, fmt)
    }
}

impl<C> IngredientRequiresReset for InternedIngredient<C>
where
    C: Configuration,
{
    const RESET_ON_NEW_REVISION: bool = false;
}

pub struct IdentityInterner<C>
where
    C: Configuration,
{
    data: PhantomData<C>,
}

impl<C> IdentityInterner<C>
where
    C: Configuration,
{
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        IdentityInterner { data: PhantomData }
    }

    pub fn intern_id<'db>(&'db self, _runtime: &'db Runtime, id: C::Data<'db>) -> crate::Id
    where
        C::Data<'db>: AsId,
    {
        id.as_id()
    }

    pub fn data_with_db<'db, DB>(&'db self, id: crate::Id, db: &'db DB) -> C::Data<'db>
    where
        DB: ?Sized,
        C::Data<'db>: LookupId<&'db DB>,
    {
        <C::Data<'db>>::lookup_id(id, db)
    }
}

impl<C> ValueStruct<C>
where
    C: Configuration,
{
    pub fn data(&self) -> &C::Data<'_> {
        // SAFETY: The lifetime of `self` is tied to the interning ingredient;
        // we never remove data without an `&mut self` access to the interning ingredient.
        unsafe { self.to_self_ref(&self.fields) }
    }

    unsafe fn to_self_ref<'db>(&'db self, fields: &'db C::Data<'static>) -> &'db C::Data<'db> {
        unsafe { std::mem::transmute(fields) }
    }
}

impl<C> AsId for ValueStruct<C>
where
    C: Configuration,
{
    fn as_id(&self) -> Id {
        self.id
    }
}
