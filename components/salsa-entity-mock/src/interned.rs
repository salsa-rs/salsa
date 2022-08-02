use crossbeam::atomic::AtomicCell;
use crossbeam::queue::SegQueue;
use std::hash::Hash;

use crate::durability::Durability;
use crate::id::AsId;
use crate::key::DependencyIndex;
use crate::runtime::local_state::QueryInputs;
use crate::runtime::Runtime;

use super::hash::FxDashMap;
use super::ingredient::Ingredient;
use super::routes::IngredientIndex;
use super::Revision;

pub trait InternedId: AsId {}
impl<T: AsId> InternedId for T {}

pub trait InternedData: Sized + Eq + Hash + Clone {}
impl<T: Eq + Hash + Clone> InternedData for T {}

#[allow(dead_code)]
pub struct InternedIngredient<Id: InternedId, Data: InternedData> {
    ingredient_index: IngredientIndex,

    // Deadlock requirement:
    //
    // We access `key_map` while holding lock on `value_map`, but not vice versa.
    key_map: FxDashMap<Data, Id>,
    value_map: FxDashMap<Id, Box<Data>>,
    counter: AtomicCell<u32>,
    reset_at: Revision,
    deleted_entries: SegQueue<Box<Data>>,
}

impl<Id, Data> InternedIngredient<Id, Data>
where
    Id: InternedId,
    Data: InternedData,
{
    pub fn new(ingredient_index: IngredientIndex) -> Self {
        Self {
            ingredient_index,
            key_map: Default::default(),
            value_map: Default::default(),
            counter: AtomicCell::default(),
            reset_at: Revision::start(),
            deleted_entries: Default::default(),
        }
    }

    #[allow(dead_code)]
    pub fn intern(&self, runtime: &Runtime, data: Data) -> Id {
        runtime.report_tracked_read(
            DependencyIndex::for_table(self.ingredient_index),
            Durability::MAX,
            self.reset_at,
        );

        if let Some(id) = self.key_map.get(&data) {
            return *id;
        }

        loop {
            let next_id = self.counter.fetch_add(1);
            let next_id = Id::from_id(crate::id::Id::from_u32(next_id));
            match self.value_map.entry(next_id) {
                // If we already have an entry with this id...
                dashmap::mapref::entry::Entry::Occupied(_) => continue,

                // Otherwise...
                dashmap::mapref::entry::Entry::Vacant(entry) => {
                    self.key_map.insert(data.clone(), next_id);
                    entry.insert(Box::new(data));
                    return next_id;
                }
            }
        }
    }

    pub(crate) fn reset_at(&self) -> Revision {
        self.reset_at
    }

    #[allow(dead_code)]
    pub fn reset(&mut self, revision: Revision) {
        assert!(revision > self.reset_at);
        self.reset_at = revision;
        self.key_map.clear();
        self.value_map.clear();
    }

    #[allow(dead_code)]
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
    /// This should only be used when you are certain that the given `id` has not (and will not)
    /// be used in the current revision. More specifically, this is used when a query `Q` executes
    /// and we can compare the entities `E_now` that it produced in this revision vs the entities
    /// `E_prev` it produced in the last revision. Any missing entities `E_prev - E_new` can be
    /// deleted.
    ///
    /// If you are wrong about this, it should not be unsafe, but unpredictable results may occur.
    pub(crate) fn delete_index(&self, id: Id) {
        match self.value_map.entry(id) {
            dashmap::mapref::entry::Entry::Vacant(_) => {
                panic!("No entry for id `{:?}`", id);
            }
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                self.key_map.remove(entry.get());

                // Careful: even though `id` ought not to have been used in this revision,
                // we don't know that for sure since users could have leaked things. If they did,
                // they may have stray references into `data`. So push the box onto the
                // "to be deleted" queue.
                //
                // To avoid this, we could include some kind of atomic counter in the `Box` that
                // gets set whenever `data` executes, so we can track if the data was accessed since
                // the last time an `&mut self` method was called. But that'd take extra storage
                // and doesn't obviously seem worth it.
                self.deleted_entries.push(entry.remove());
            }
        }
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

    fn inputs(&self, _key_index: crate::Id) -> Option<QueryInputs> {
        None
    }
}
