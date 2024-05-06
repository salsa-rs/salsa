use std::{fmt, hash::Hash};

use crate::{
    cycle::CycleRecoveryStrategy,
    id::AsId,
    ingredient::{fmt_index, Ingredient, IngredientRequiresReset},
    ingredient_list::IngredientList,
    interned::InternedIngredient,
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::{local_state::QueryOrigin, Runtime},
    salsa_struct::SalsaStructInDb,
    Database, Durability, Event, Id, IngredientIndex, Revision,
};

use self::struct_map::{StructMap, Update};
pub use self::tracked_field::TrackedFieldIngredient;

mod struct_map;
mod tracked_field;

// ANCHOR: Configuration
/// Trait that defines the key properties of a tracked struct.
/// Implemented by the `#[salsa::tracked]` macro when applied
/// to a struct.
pub trait Configuration {
    /// A (possibly empty) tuple of the fields for this struct.
    type Fields<'db>;

    /// A array of [`Revision`][] values, one per each of the value fields.
    /// When a struct is re-recreated in a new revision, the corresponding
    /// entries for each field are updated to the new revision if their
    /// values have changed (or if the field is marked as `#[no_eq]`).
    type Revisions;

    fn id_fields(fields: &Self::Fields<'_>) -> impl Hash;

    /// Access the revision of a given value field.
    /// `field_index` will be between 0 and the number of value fields.
    fn revision(revisions: &Self::Revisions, field_index: u32) -> Revision;

    /// Create a new value revision array where each element is set to `current_revision`.
    fn new_revisions(current_revision: Revision) -> Self::Revisions;

    /// Update the field data and, if the value has changed,
    /// the appropriate entry in the `revisions` array.
    ///
    /// # Safety requirements and conditions
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
    );
}
// ANCHOR_END: Configuration

pub trait TrackedStructInDb<DB: ?Sized + Database>: SalsaStructInDb<DB> {
    /// Converts the identifier for this tracked struct into a `DatabaseKeyIndex`.
    fn database_key_index(db: &DB, id: Id) -> DatabaseKeyIndex;
}

/// Created for each tracked struct.
/// This ingredient only stores the "id" fields.
/// It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked struct id.
/// The value fields are stored in [`crate::function::FunctionIngredient`] instances keyed by the tracked struct id.
/// Unlike normal interners, tracked struct indices can be deleted and reused aggressively:
/// when a tracked function re-executes,
/// any tracked structs that it created before but did not create this time can be deleted.
pub struct TrackedStructIngredient<C>
where
    C: Configuration,
{
    interned: InternedIngredient<TrackedStructKey>,

    struct_map: struct_map::StructMap<C>,

    /// A list of each tracked function `f` whose key is this
    /// tracked struct.
    ///
    /// Whenever an instance `i` of this struct is deleted,
    /// each of these functions will be notified
    /// so they can remove any data tied to that instance.
    dependent_fns: IngredientList,

    debug_name: &'static str,
}

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Copy, Clone)]
struct TrackedStructKey {
    query_key: DatabaseKeyIndex,
    disambiguator: Disambiguator,
    data_hash: u64,
}

impl crate::interned::Configuration for TrackedStructKey {
    type Data = TrackedStructKey;
}

// ANCHOR: TrackedStructValue
#[derive(Debug)]
pub struct TrackedStructValue<C>
where
    C: Configuration,
{
    /// Index of the struct ingredient.
    struct_ingredient_index: IngredientIndex,

    /// The id of this struct in the ingredient.
    id: Id,

    /// The durability minimum durability of all inputs consumed
    /// by the creator query prior to creating this tracked struct.
    /// If any of those inputs changes, then the creator query may
    /// create this struct with different values.
    durability: Durability,

    /// The revision when this entity was most recently created.
    /// Typically the current revision.
    /// Used to detect "leaks" outside of the salsa system -- i.e.,
    /// access to tracked structs that have not (yet?) been created in the
    /// current revision. This should be impossible within salsa queries
    /// but it can happen through "leaks" like thread-local data or storing
    /// values outside of the root salsa query.
    created_at: Revision,

    /// Fields of this tracked struct. They can change across revisions,
    /// but they do not change within a particular revision.
    fields: C::Fields<'static>,

    /// The revision information for each field: when did this field last change.
    /// When tracked structs are re-created, this revision may be updated to the
    /// current revision if the value is different.
    revisions: C::Revisions,
}
// ANCHOR_END: TrackedStructValue

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Copy, Clone)]
pub struct Disambiguator(pub u32);

impl<C> TrackedStructIngredient<C>
where
    C: Configuration,
{
    unsafe fn to_static<'db>(&'db self, fields: C::Fields<'db>) -> C::Fields<'static> {
        unsafe { std::mem::transmute(fields) }
    }

    unsafe fn to_self_ptr<'db>(&'db self, fields: *mut C::Fields<'static>) -> *mut C::Fields<'db> {
        unsafe { std::mem::transmute(fields) }
    }

    pub fn new(index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            interned: InternedIngredient::new(index, debug_name),
            struct_map: StructMap::new(),
            dependent_fns: IngredientList::new(),
            debug_name,
        }
    }

    fn struct_ingredient_index(&self) -> IngredientIndex {
        self.interned.ingredient_index()
    }

    pub fn new_field_ingredient(
        &self,
        field_ingredient_index: IngredientIndex,
        field_index: u32,
        field_debug_name: &'static str,
    ) -> TrackedFieldIngredient<C> {
        assert_eq!(
            field_ingredient_index.as_u32() - self.struct_ingredient_index().as_u32() - 1,
            field_index,
        );

        TrackedFieldIngredient::<C> {
            ingredient_index: field_ingredient_index,
            field_index,
            struct_map: self.struct_map.view(),
            struct_debug_name: self.debug_name,
            field_debug_name,
        }
    }

    pub fn database_key_index(&self, id: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.interned.ingredient_index(),
            key_index: id.as_id(),
        }
    }

    pub fn new_struct<'db>(
        &'db self,
        runtime: &'db Runtime,
        fields: C::Fields<'db>,
    ) -> &'db TrackedStructValue<C> {
        let data_hash = crate::hash::hash(&C::id_fields(&fields));

        let (query_key, current_deps, disambiguator) = runtime.disambiguate_entity(
            self.interned.ingredient_index(),
            self.interned.reset_at(),
            data_hash,
        );

        let entity_key = TrackedStructKey {
            query_key,
            disambiguator,
            data_hash,
        };
        let (id, new_id) = self.interned.intern_full(runtime, entity_key);
        runtime.add_output(self.database_key_index(id).into());

        let current_revision = runtime.current_revision();
        if new_id {
            self.struct_map.insert(
                runtime,
                TrackedStructValue {
                    id,
                    struct_ingredient_index: self.struct_ingredient_index(),
                    created_at: current_revision,
                    durability: current_deps.durability,
                    fields: unsafe { self.to_static(fields) },
                    revisions: C::new_revisions(current_deps.changed_at),
                },
            )
        } else {
            match self.struct_map.update(runtime, id) {
                Update::Current(r) => {
                    // All inputs up to this point were previously
                    // observed to be green and this struct was already
                    // verified. Therefore, the durability ought not to have
                    // changed (nor the field values, but the user could've
                    // done something stupid, so we can't *assert* this is true).
                    assert!(r.durability == current_deps.durability);

                    r
                }
                Update::Outdated(mut data_ref) => {
                    let data = &mut *data_ref;

                    // SAFETY: We assert that the pointer to `data.revisions`
                    // is a pointer into the database referencing a value
                    // from a previous revision. As such, it continues to meet
                    // its validity invariant and any owned content also continues
                    // to meet its safety invariant.
                    unsafe {
                        C::update_fields(
                            current_revision,
                            &mut data.revisions,
                            self.to_self_ptr(std::ptr::addr_of_mut!(data.fields)),
                            fields,
                        );
                    }
                    if current_deps.durability < data.durability {
                        data.revisions = C::new_revisions(current_revision);
                    }
                    data.created_at = current_revision;
                    data.durability = current_deps.durability;
                    data_ref.freeze()
                }
            }
        }
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
        db.salsa_event(Event {
            runtime_id: db.runtime().id(),
            kind: crate::EventKind::DidDiscard {
                key: self.database_key_index(id),
            },
        });

        self.interned.delete_index(id);
        self.struct_map.delete(id);

        for dependent_fn in self.dependent_fns.iter() {
            db.salsa_struct_deleted(dependent_fn, id.as_id());
        }
    }

    /// Adds a dependent function (one keyed by this tracked struct) to our list.
    /// When instances of this struct are deleted, these dependent functions
    /// will be notified.
    pub fn register_dependent_fn(&self, index: IngredientIndex) {
        self.dependent_fns.push(index);
    }
}

impl<DB: ?Sized, C> Ingredient<DB> for TrackedStructIngredient<C>
where
    DB: Database,
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.interned.ingredient_index()
    }

    fn maybe_changed_after(&self, db: &DB, input: DependencyIndex, revision: Revision) -> bool {
        self.interned.maybe_changed_after(db, input, revision)
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        <_ as Ingredient<DB>>::cycle_recovery_strategy(&self.interned)
    }

    fn origin(&self, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output<'db>(
        &'db self,
        db: &'db DB,
        _executor: DatabaseKeyIndex,
        output_key: Option<crate::Id>,
    ) {
        let runtime = db.runtime();
        let output_key = output_key.unwrap();
        self.struct_map.validate(runtime, output_key);
    }

    fn remove_stale_output(
        &self,
        db: &DB,
        _executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        // This method is called when, in prior revisions,
        // `executor` creates a tracked struct `salsa_output_key`,
        // but it did not in the current revision.
        // In that case, we can delete `stale_output_key` and any data associated with it.
        self.delete_entity(db.as_salsa_database(), stale_output_key.unwrap());
    }

    fn reset_for_new_revision(&mut self) {
        self.interned.clear_deleted_indices();
        self.struct_map.drop_deleted_entries();
    }

    fn salsa_struct_deleted(&self, _db: &DB, _id: crate::Id) {
        panic!("unexpected call: interned ingredients do not register for salsa struct deletion events");
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(self.debug_name, index, fmt)
    }
}

impl<C> IngredientRequiresReset for TrackedStructIngredient<C>
where
    C: Configuration,
{
    const RESET_ON_NEW_REVISION: bool = true;
}

impl<C> TrackedStructValue<C>
where
    C: Configuration,
{
    /// The id of this struct in the ingredient.
    pub fn id(&self) -> Id {
        self.id
    }

    /// Access to this value field.
    /// Note that this function returns the entire tuple of value fields.
    /// The caller is responible for selecting the appropriate element.
    pub fn field<'db>(&'db self, runtime: &'db Runtime, field_index: u32) -> &'db C::Fields<'db> {
        let field_ingredient_index = IngredientIndex::from(
            self.struct_ingredient_index.as_usize() + field_index as usize + 1,
        );
        let changed_at = C::revision(&self.revisions, field_index);

        runtime.report_tracked_read(
            DependencyIndex {
                ingredient_index: field_ingredient_index,
                key_index: Some(self.id.as_id()),
            },
            self.durability,
            changed_at,
        );

        unsafe { self.to_self_ref(&self.fields) }
    }

    unsafe fn to_self_ref<'db>(&'db self, fields: &'db C::Fields<'static>) -> &'db C::Fields<'db> {
        unsafe { std::mem::transmute(fields) }
    }
}
