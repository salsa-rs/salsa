use std::{fmt, hash::Hash, sync::Arc};

use crossbeam::queue::SegQueue;

use crate::{
    cycle::CycleRecoveryStrategy,
    hash::FxDashMap,
    id::AsId,
    ingredient::{fmt_index, Ingredient, IngredientRequiresReset},
    ingredient_list::IngredientList,
    interned::{InternedId, InternedIngredient},
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::{local_state::QueryOrigin, Runtime},
    salsa_struct::SalsaStructInDb,
    Database, Durability, Event, IngredientIndex, Revision,
};

pub use self::tracked_field::TrackedFieldIngredient;

mod tracked_field;

// ANCHOR: Configuration
/// Trait that defines the key properties of a tracked struct.
/// Implemented by the `#[salsa::tracked]` macro when applied
/// to a struct.
pub trait Configuration {
    /// The id type used to define instances of this struct.
    /// The [`TrackedStructIngredient`][] contains the interner
    /// that will create the id values.
    type Id: InternedId;

    /// A (possibly empty) tuple of the fields for this struct.
    type Fields;

    /// A array of [`Revision`][] values, one per each of the value fields.
    /// When a struct is re-recreated in a new revision, the
    type Revisions;

    fn id_fields(fields: &Self::Fields) -> impl Hash;

    /// Access the revision of a given value field.
    /// `field_index` will be between 0 and the number of value fields.
    fn revision(revisions: &Self::Revisions, field_index: u32) -> Revision;

    /// Create a new value revision array where each element is set to `current_revision`.
    fn new_revisions(current_revision: Revision) -> Self::Revisions;

    /// Update an existing value revision array `revisions`,
    /// given the tuple of the old values (`old_value`)
    /// and the tuple of the values (`new_value`).
    /// If a value has changed, then its element is
    /// updated to `current_revision`.
    fn update_revisions(
        current_revision: Revision,
        old_value: &Self::Fields,
        new_value: &Self::Fields,
        revisions: &mut Self::Revisions,
    );
}
// ANCHOR_END: Configuration

pub trait TrackedStructInDb<DB: ?Sized + Database>: SalsaStructInDb<DB> {
    /// Converts the identifier for this tracked struct into a `DatabaseKeyIndex`.
    fn database_key_index(self, db: &DB) -> DatabaseKeyIndex;
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
    interned: InternedIngredient<C::Id, TrackedStructKey>,

    entity_data: Arc<FxDashMap<C::Id, Box<TrackedStructValue<C>>>>,

    /// A list of each tracked function `f` whose key is this
    /// tracked struct.
    ///
    /// Whenever an instance `i` of this struct is deleted,
    /// each of these functions will be notified
    /// so they can remove any data tied to that instance.
    dependent_fns: IngredientList,

    /// When specific entities are deleted, their data is added
    /// to this vector rather than being immediately freed. This is because we may` have
    /// references to that data floating about that are tied to the lifetime of some
    /// `&db` reference. This queue itself is not freed until we have an `&mut db` reference,
    /// guaranteeing that there are no more references to it.
    deleted_entries: SegQueue<Box<TrackedStructValue<C>>>,

    debug_name: &'static str,
}

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Copy, Clone)]
struct TrackedStructKey {
    query_key: DatabaseKeyIndex,
    disambiguator: Disambiguator,
    data_hash: u64,
}

// ANCHOR: TrackedStructValue
#[derive(Debug)]
struct TrackedStructValue<C>
where
    C: Configuration,
{
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
    fields: C::Fields,

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
    pub fn new(index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            interned: InternedIngredient::new(index, debug_name),
            entity_data: Default::default(),
            dependent_fns: IngredientList::new(),
            deleted_entries: SegQueue::default(),
            debug_name,
        }
    }

    pub fn new_field_ingredient(
        &self,
        field_ingredient_index: IngredientIndex,
        field_index: u32,
        field_debug_name: &'static str,
    ) -> TrackedFieldIngredient<C> {
        TrackedFieldIngredient {
            ingredient_index: field_ingredient_index,
            field_index,
            entity_data: self.entity_data.clone(),
            struct_debug_name: self.debug_name,
            field_debug_name,
        }
    }

    pub fn database_key_index(&self, id: C::Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.interned.ingredient_index(),
            key_index: id.as_id(),
        }
    }

    pub fn new_struct(&self, runtime: &Runtime, fields: C::Fields) -> C::Id {
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
            let old_value = self.entity_data.insert(
                id,
                Box::new(TrackedStructValue {
                    created_at: current_revision,
                    durability: current_deps.durability,
                    fields,
                    revisions: C::new_revisions(current_deps.changed_at),
                }),
            );
            assert!(old_value.is_none());
        } else {
            let mut data = self.entity_data.get_mut(&id).unwrap();
            let data = &mut *data;
            if current_deps.durability < data.durability {
                data.revisions = C::new_revisions(current_revision);
            } else {
                C::update_revisions(current_revision, &data.fields, &fields, &mut data.revisions);
            }
            data.created_at = current_revision;
            data.durability = current_deps.durability;

            // Subtle but important: we *always* update the values of the fields,
            // even if they are `==` to the old values. This is because the `==`
            // operation might not mean tha tthe fields are bitwise equal, and we
            // want to take the new value.
            data.fields = fields;
        }

        id
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
    pub(crate) fn delete_entity(&self, db: &dyn crate::Database, id: C::Id) {
        db.salsa_event(Event {
            runtime_id: db.runtime().id(),
            kind: crate::EventKind::DidDiscard {
                key: self.database_key_index(id),
            },
        });

        self.interned.delete_index(id);
        if let Some((_, data)) = self.entity_data.remove(&id) {
            self.deleted_entries.push(data);
        }

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

    fn mark_validated_output(
        &self,
        _db: &DB,
        _executor: DatabaseKeyIndex,
        _output_key: Option<crate::Id>,
    ) {
        // FIXME
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
        let stale_output_key: C::Id = <C::Id>::from_id(stale_output_key.unwrap());
        self.delete_entity(db.as_salsa_database(), stale_output_key);
    }

    fn reset_for_new_revision(&mut self) {
        self.interned.clear_deleted_indices();
        std::mem::take(&mut self.deleted_entries);
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
