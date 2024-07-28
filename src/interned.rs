use crossbeam::atomic::AtomicCell;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::alloc::Alloc;
use crate::durability::Durability;
use crate::id::AsId;
use crate::ingredient::fmt_index;
use crate::key::DependencyIndex;
use crate::plumbing::Jar;
use crate::zalsa::IngredientIndex;
use crate::zalsa_local::QueryOrigin;
use crate::{Database, DatabaseKeyIndex, Id};

use super::hash::FxDashMap;
use super::ingredient::Ingredient;
use super::Revision;

pub trait Configuration: Sized + 'static {
    const DEBUG_NAME: &'static str;

    /// The type of data being interned
    type Data<'db>: InternedData + Send + Sync;

    /// The end user struct
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
    unsafe fn struct_from_raw<'db>(ptr: NonNull<Value<Self>>) -> Self::Struct<'db>;

    /// Deref the struct to yield the underlying value struct.
    /// Since we are still part of the `'db` lifetime in which the struct was created,
    /// this deref is safe, and the value-struct fields are immutable and verified.
    fn deref_struct(s: Self::Struct<'_>) -> &Value<Self>;
}

pub trait InternedData: Sized + Eq + Hash + Clone {}
impl<T: Eq + Hash + Clone> InternedData for T {}

pub struct JarImpl<C: Configuration> {
    phantom: PhantomData<C>,
}

/// The interned ingredient has the job of hashing values of type `Data` to produce an `Id`.
/// It used to store interned structs but also to store the id fields of a tracked struct.
/// Interned values endure until they are explicitly removed in some way.
pub struct IngredientImpl<C: Configuration> {
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,

    /// Maps from data to the existing interned id for that data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    key_map: FxDashMap<C::Data<'static>, Id>,

    /// Maps from an interned id to its data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    value_map: FxDashMap<Id, Alloc<Value<C>>>,

    /// counter for the next id.
    counter: AtomicCell<u32>,

    /// Stores the revision when this interned ingredient was last cleared.
    /// You can clear an interned table at any point, deleting all its entries,
    /// but that will make anything dependent on those entries dirty and in need
    /// of being recomputed.
    reset_at: Revision,
}

/// Struct storing the interned fields.
pub struct Value<C>
where
    C: Configuration,
{
    id: Id,
    fields: C::Data<'static>,
}

impl<C: Configuration> Default for JarImpl<C> {
    fn default() -> Self {
        Self {
            phantom: PhantomData,
        }
    }
}

impl<C: Configuration> Jar for JarImpl<C> {
    fn create_ingredients(&self, first_index: IngredientIndex) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(IngredientImpl::<C>::new(first_index)) as _]
    }
}

impl<C> IngredientImpl<C>
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

    pub fn intern_id<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        data: C::Data<'db>,
    ) -> crate::Id {
        C::deref_struct(self.intern(db, data)).as_id()
    }

    /// Intern data to a unique reference.
    pub fn intern<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        data: C::Data<'db>,
    ) -> C::Struct<'db> {
        let zalsa_local = db.zalsa_local();
        zalsa_local.report_tracked_read(
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
                let value = self.value_map.entry(next_id).or_insert(Alloc::new(Value {
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

    /// Lookup the fields from an interned struct.
    /// Note that this is not "leaking" since no dependency edge is required.
    pub fn fields<'db>(&'db self, s: C::Struct<'db>) -> &'db C::Data<'db> {
        C::deref_struct(s).data()
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

impl<C> Ingredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn maybe_changed_after(
        &self,
        _db: &dyn Database,
        _input: Option<Id>,
        revision: Revision,
    ) -> bool {
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
        _db: &dyn Database,
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
        _db: &dyn Database,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "remove_stale_output({:?}, {:?}): interned ids are not outputs",
            executor, stale_output_key
        );
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    fn reset_for_new_revision(&mut self) {
        // Interned ingredients do not, normally, get deleted except when they are "reset" en masse.
        // There ARE methods (e.g., `clear_deleted_entries` and `remove`) for deleting individual
        // items, but those are only used for tracked struct ingredients.
        panic!("unexpected call to `reset_for_new_revision`")
    }

    fn salsa_struct_deleted(&self, _db: &dyn Database, _id: crate::Id) {
        panic!("unexpected call: interned ingredients do not register for salsa struct deletion events");
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
            .field("index", &self.ingredient_index)
            .finish()
    }
}

impl<C> Value<C>
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

impl<C> AsId for Value<C>
where
    C: Configuration,
{
    fn as_id(&self) -> Id {
        self.id
    }
}
