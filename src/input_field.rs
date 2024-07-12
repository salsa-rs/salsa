use crate::cycle::CycleRecoveryStrategy;
use crate::id::{AsId, FromId};
use crate::ingredient::{fmt_index, Ingredient, IngredientRequiresReset};
use crate::input::Configuration;
use crate::runtime::local_state::QueryOrigin;
use crate::runtime::StampedValue;
use crate::storage::IngredientIndex;
use crate::{Database, DatabaseKeyIndex, Durability, Id, Revision, Runtime};
use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::fmt;

pub trait InputFieldData: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> InputFieldData for T {}

/// Ingredient used to represent the fields of a `#[salsa::input]`.
///
/// These fields can only be mutated by a call to a setter with an `&mut`
/// reference to the database, and therefore cannot be mutated during a tracked
/// function or in parallel.
/// However for on-demand inputs to work the fields must be able to be set via
/// a shared reference, so some locking is required.
/// Altogether this makes the implementation somewhat simpler than tracked
/// structs.
pub struct FieldIngredientImpl<C: Configuration, F: InputFieldData> {
    index: IngredientIndex,
    map: DashMap<C::Id, Box<StampedValue<F>>>,
    debug_name: &'static str,
}

impl<C, F> FieldIngredientImpl<C, F>
where
    C: Configuration,
    F: InputFieldData,
{
    pub fn new(index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            index,
            map: Default::default(),
            debug_name,
        }
    }

    pub fn store_mut(
        &mut self,
        runtime: &Runtime,
        key: C::Id,
        value: F,
        durability: Durability,
    ) -> Option<F> {
        let revision = runtime.current_revision();
        let stamped_value = Box::new(StampedValue {
            value,
            durability,
            changed_at: revision,
        });

        self.map
            .insert(key, stamped_value)
            .map(|old_value| old_value.value)
    }

    /// Set the field of a new input.
    ///
    /// This function panics if the field has ever been set before.
    pub fn store_new(&self, runtime: &Runtime, key: C::Id, value: F, durability: Durability) {
        let revision = runtime.current_revision();
        let stamped_value = Box::new(StampedValue {
            value,
            durability,
            changed_at: revision,
        });

        match self.map.entry(key) {
            Entry::Occupied(_) => {
                panic!("attempted to set field of existing input using `store_new`, use `store_mut` instead");
            }
            Entry::Vacant(entry) => {
                entry.insert(stamped_value);
            }
        }
    }

    pub fn fetch<'db>(&'db self, runtime: &'db Runtime, key: C::Id) -> &F {
        let StampedValue {
            value,
            durability,
            changed_at,
        } = &**self.map.get(&key).unwrap();

        runtime.report_tracked_read(
            self.database_key_index(key).into(),
            *durability,
            *changed_at,
        );

        // SAFETY:
        // The value is stored in a box so internal moves in the dashmap don't
        // invalidate the reference to the value inside the box.
        // Values are only removed or altered when we have `&mut self`.
        unsafe { transmute_lifetime(self, value) }
    }

    fn database_key_index(&self, key: C::Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.index,
            key_index: key.as_id(),
        }
    }
}

/// More limited wrapper around transmute that copies lifetime from `a` to `b`.
///
/// # Safety condition
///
/// `b` must be owned by `a`
unsafe fn transmute_lifetime<'a, 'b, A, B>(_a: &'a A, b: &'b B) -> &'a B {
    std::mem::transmute(b)
}

impl<C, F> Ingredient for FieldIngredientImpl<C, F>
where
    C: Configuration,
    F: InputFieldData,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.index
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        CycleRecoveryStrategy::Panic
    }

    fn maybe_changed_after(
        &self,
        _db: &dyn Database,
        input: Option<Id>,
        revision: Revision,
    ) -> bool {
        let key = C::Id::from_id(input.unwrap());
        self.map.get(&key).unwrap().changed_at > revision
    }

    fn origin(&self, _key_index: Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &dyn Database,
        _executor: DatabaseKeyIndex,
        _output_key: Option<Id>,
    ) {
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        _executor: DatabaseKeyIndex,
        _stale_output_key: Option<Id>,
    ) {
    }

    fn salsa_struct_deleted(&self, _db: &dyn Database, _id: Id) {
        panic!("unexpected call: input fields are never deleted");
    }

    fn reset_for_new_revision(&mut self) {
        panic!("unexpected call: input fields don't register for resets");
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(self.debug_name, index, fmt)
    }
}

impl<C, F> IngredientRequiresReset for FieldIngredientImpl<C, F>
where
    C: Configuration,
    F: InputFieldData,
{
    const RESET_ON_NEW_REVISION: bool = false;
}

impl<C, F> std::fmt::Debug for FieldIngredientImpl<C, F>
where
    C: Configuration,
    F: InputFieldData,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.index)
            .finish()
    }
}
