use std::marker::PhantomData;

use crate::{ingredient::Ingredient, zalsa::IngredientIndex, Database, Id};

use super::{Configuration, Value};

/// Created for each tracked struct.
///
/// This ingredient only stores the "id" fields.
/// It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked struct id.
/// The value fields are stored in [`crate::function::FunctionIngredient`] instances keyed by the tracked struct id.
/// Unlike normal interners, tracked struct indices can be deleted and reused aggressively:
/// when a tracked function re-executes,
/// any tracked structs that it created before but did not create this time can be deleted.
pub struct FieldIngredientImpl<C>
where
    C: Configuration,
{
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,
    field_index: usize,
    phantom: PhantomData<fn() -> Value<C>>,
}

impl<C> FieldIngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn new(struct_index: IngredientIndex, field_index: usize) -> Self {
        Self {
            ingredient_index: struct_index.successor(field_index),
            field_index,
            phantom: PhantomData,
        }
    }
}

impl<C> Ingredient for FieldIngredientImpl<C>
where
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn cycle_recovery_strategy(&self) -> crate::cycle::CycleRecoveryStrategy {
        crate::cycle::CycleRecoveryStrategy::Panic
    }

    fn maybe_changed_after<'db>(
        &'db self,
        db: &'db dyn Database,
        input: Id,
        revision: crate::Revision,
    ) -> bool {
        let zalsa = db.zalsa();
        let data = <super::IngredientImpl<C>>::data(zalsa.table(), input);
        let field_changed_at = data.revisions[self.field_index];
        field_changed_at > revision
    }

    fn origin(
        &self,
        _db: &dyn Database,
        _key_index: crate::Id,
    ) -> Option<crate::zalsa_local::QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &dyn Database,
        _executor: crate::DatabaseKeyIndex,
        _output_key: Id,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        _executor: crate::DatabaseKeyIndex,
        _stale_output_key: Id,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    fn reset_for_new_revision(&mut self) {
        panic!("tracked field ingredients do not require reset")
    }

    fn fmt_index(&self, index: Id, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "{}.{}({:?})",
            C::DEBUG_NAME,
            C::FIELD_DEBUG_NAMES[self.field_index],
            index
        )
    }

    fn debug_name(&self) -> &'static str {
        C::FIELD_DEBUG_NAMES[self.field_index]
    }

    fn accumulated<'db>(
        &'db self,
        _db: &'db dyn Database,
        _key_index: Id,
    ) -> Option<&'db crate::accumulator::accumulated_map::AccumulatedMap> {
        None
    }
}

impl<C> std::fmt::Debug for FieldIngredientImpl<C>
where
    C: Configuration,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("ingredient_index", &self.ingredient_index)
            .field("field_index", &self.field_index)
            .finish()
    }
}
