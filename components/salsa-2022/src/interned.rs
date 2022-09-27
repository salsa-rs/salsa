use crossbeam::atomic::AtomicCell;
use crossbeam::queue::SegQueue;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;

use crate::durability::Durability;
use crate::id::AsId;
use crate::ingredient::{fmt_index, IngredientRequiresReset};
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryOrigin;
use crate::runtime::Runtime;
use crate::DatabaseKeyIndex;

use super::hash::FxDashMap;
use super::ingredient::Ingredient;
use super::routes::IngredientIndex;
use super::Revision;

pub trait InternedId: AsId {}
impl<T: AsId> InternedId for T {}

pub trait InternedData: Sized + Eq + Hash + Clone {}
impl<T: Eq + Hash + Clone> InternedData for T {}

/// The interned ingredient has the job of hashing values of type `Data` to produce an `Id`.
/// It used to store interned structs but also to store the id fields of a tracked struct.
/// Interned values endure until they are explicitly removed in some way.
pub struct InternedIngredient<Id: InternedId, Data: InternedData> {
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,

    /// Maps from data to the existing interned id for that data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    key_map: FxDashMap<Data, Id>,

    /// Maps from an interned id to its data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    value_map: FxDashMap<Id, Box<Data>>,

    /// counter for the next id.
    counter: AtomicCell<u32>,

    /// Stores the revision when this interned ingredient was last cleared.
    /// You can clear an interned table at any point, deleting all its entries,
    /// but that will make anything dependent on those entries dirty and in need
    /// of being recomputed.
    reset_at: Revision,

    /// When specific entries are deleted from the interned table, their data is added
    /// to this vector rather than being immediately freed. This is because we may` have
    /// references to that data floating about that are tied to the lifetime of some
    /// `&db` reference. This queue itself is not freed until we have an `&mut db` reference,
    /// guaranteeing that there are no more references to it.
    deleted_entries: SegQueue<Box<Data>>,

    debug_name: &'static str,
}

impl<Id, Data> InternedIngredient<Id, Data>
where
    Id: InternedId,
    Data: InternedData,
{
    pub fn new(ingredient_index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            ingredient_index,
            key_map: Default::default(),
            value_map: Default::default(),
            counter: AtomicCell::default(),
            reset_at: Revision::start(),
            deleted_entries: Default::default(),
            debug_name,
        }
    }

    pub fn intern(&self, runtime: &Runtime, data: Data) -> Id {
        runtime.report_tracked_read(
            DependencyIndex::for_table(self.ingredient_index),
            Durability::MAX,
            self.reset_at,
        );

        // Optimisation to only get read lock on the map if the data has already
        // been interned.
        if let Some(id) = self.key_map.get(&data) {
            return *id;
        }

        match self.key_map.entry(data.clone()) {
            // Data has been interned by a racing call, use that ID instead
            dashmap::mapref::entry::Entry::Occupied(entry) => *entry.get(),
            // We won any races so should intern the data
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let next_id = self.counter.fetch_add(1);
                let next_id = Id::from_id(crate::id::Id::from_u32(next_id));
                let old_value = self.value_map.insert(next_id, Box::new(data));
                assert!(
                    old_value.is_none(),
                    "next_id is guaranteed to be unique, bar overflow"
                );
                entry.insert(next_id);
                next_id
            }
        }
    }

    pub(crate) fn reset_at(&self) -> Revision {
        self.reset_at
    }

    pub fn reset(&mut self, revision: Revision) {
        assert!(revision > self.reset_at);
        self.reset_at = revision;
        self.key_map.clear();
        self.value_map.clear();
    }

    #[track_caller]
    pub fn data<'db>(&'db self, runtime: &'db Runtime, id: Id) -> &'db Data {
        runtime.report_tracked_read(
            DependencyIndex::for_table(self.ingredient_index),
            Durability::MAX,
            self.reset_at,
        );

        let data = match self.value_map.get(&id) {
            Some(d) => d,
            None => {
                panic!("no data found for id `{:?}`", id)
            }
        };

        // Unsafety clause:
        //
        // * Values are only removed or altered when we have `&mut self`
        unsafe { transmute_lifetime(self, &**data) }
    }

    /// Get the ingredient index for this table.
    pub(super) fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    /// Deletes an index from the interning table, making it available for re-use.
    ///
    /// # Warning
    ///
    /// This should only be used when you are certain that:
    ///  1. The given `id` has not (and will not) be used in the current revision.
    ///  2. The interned data corresponding to `id` will not be interned in this revision.
    ///
    /// More specifically, this is used when a query `Q` executes and we can compare the
    /// entities `E_now` that it produced in this revision vs the entities `E_prev` it
    /// produced in the last revision. Any missing entities `E_prev - E_new` can be deleted.
    ///
    /// If you are wrong about this, it should not be unsafe, but unpredictable results may occur.
    pub(crate) fn delete_index(&self, id: Id) {
        let (_, key) = self
            .value_map
            .remove(&id)
            .unwrap_or_else(|| panic!("No entry for id `{:?}`", id));

        self.key_map.remove(&key);
        // Careful: even though `id` ought not to have been used in this revision,
        // we don't know that for sure since users could have leaked things. If they did,
        // they may have stray references into `data`. So push the box onto the
        // "to be deleted" queue.
        //
        // To avoid this, we could include some kind of atomic counter in the `Box` that
        // gets set whenever `data` executes, so we can track if the data was accessed since
        // the last time an `&mut self` method was called. But that'd take extra storage
        // and doesn't obviously seem worth it.
        self.deleted_entries.push(key);
    }

    pub(crate) fn clear_deleted_indices(&mut self) {
        std::mem::take(&mut self.deleted_entries);
    }
}

// Returns `u` but with the lifetime of `t`.
//
// Safe if you know that data at `u` will remain shared
// until the reference `t` expires.
unsafe fn transmute_lifetime<'t, 'u, T, U>(_t: &'t T, u: &'u U) -> &'t U {
    std::mem::transmute(u)
}

impl<DB: ?Sized, Id, Data> Ingredient<DB> for InternedIngredient<Id, Data>
where
    Id: InternedId,
    Data: InternedData,
{
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
        fmt_index(self.debug_name, index, fmt)
    }
}

impl<Id, Data> IngredientRequiresReset for InternedIngredient<Id, Data>
where
    Id: InternedId,
    Data: InternedData,
{
    const RESET_ON_NEW_REVISION: bool = false;
}

pub struct IdentityInterner<Id: AsId> {
    data: PhantomData<Id>,
}

impl<Id: AsId> IdentityInterner<Id> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        IdentityInterner { data: PhantomData }
    }

    pub fn intern(&self, _runtime: &Runtime, id: Id) -> Id {
        id
    }

    pub fn data(&self, _runtime: &Runtime, id: Id) -> (Id,) {
        (id,)
    }
}
