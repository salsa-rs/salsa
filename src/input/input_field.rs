use crate::cycle::CycleRecoveryStrategy;
use crate::id::AsId;
use crate::ingredient::{fmt_index, Ingredient, IngredientRequiresReset};
use crate::input::Configuration;
use crate::runtime::local_state::QueryOrigin;
use crate::storage::IngredientIndex;
use crate::{Database, DatabaseKeyIndex, Id, Revision};
use std::fmt;

use super::struct_map::StructMap;

pub trait FieldData: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> FieldData for T {}

/// Ingredient used to represent the fields of a `#[salsa::input]`.
///
/// These fields can only be mutated by a call to a setter with an `&mut`
/// reference to the database, and therefore cannot be mutated during a tracked
/// function or in parallel.
/// However for on-demand inputs to work the fields must be able to be set via
/// a shared reference, so some locking is required.
/// Altogether this makes the implementation somewhat simpler than tracked
/// structs.
pub struct FieldIngredientImpl<C: Configuration> {
    index: IngredientIndex,
    field_index: usize,
    struct_map: StructMap<C>,
}

impl<C> FieldIngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn new(
        struct_index: IngredientIndex,
        field_index: usize,
        struct_map: StructMap<C>,
    ) -> Self {
        Self {
            index: struct_index + field_index,
            field_index,
            struct_map,
        }
    }

    fn database_key_index(&self, key: C::Struct) -> DatabaseKeyIndex {
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

impl<C> Ingredient for FieldIngredientImpl<C>
where
    C: Configuration,
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
        let input = input.unwrap();
        self.struct_map.get(input).stamps[self.field_index].changed_at > revision
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
        fmt_index(C::FIELD_DEBUG_NAMES[self.field_index as usize], index, fmt)
    }
}

impl<C> IngredientRequiresReset for FieldIngredientImpl<C>
where
    C: Configuration,
{
    const RESET_ON_NEW_REVISION: bool = false;
}

impl<C> std::fmt::Debug for FieldIngredientImpl<C>
where
    C: Configuration,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.index)
            .finish()
    }
}
