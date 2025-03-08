use std::marker::PhantomData;

use crate::{function::VerifyResult, ingredient::Ingredient, zalsa::IngredientIndex, Database, Id};

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

    /// The absolute index of this field on the tracked struct.
    field_index: usize,

    phantom: PhantomData<fn() -> Value<C>>,
}

impl<C> FieldIngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn new(field_index: usize, ingredient_index: IngredientIndex) -> Self {
        Self {
            field_index,
            ingredient_index,
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

    unsafe fn maybe_changed_after<'db>(
        &'db self,
        db: &'db dyn Database,
        input: Id,
        revision: crate::Revision,
    ) -> VerifyResult {
        let zalsa = db.zalsa();
        let data = <super::IngredientImpl<C>>::data(zalsa.table(), input);
        let field_changed_at = data.revisions[self.field_index];
        VerifyResult::changed_if(field_changed_at > revision)
    }

    fn is_provisional_cycle_head<'db>(&'db self, _db: &'db dyn Database, _input: Id) -> bool {
        false
    }

    fn wait_for(&self, _db: &dyn Database, _key_index: Id) -> bool {
        true
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
        _output_key: crate::Id,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        _executor: crate::DatabaseKeyIndex,
        _stale_output_key: crate::Id,
        _provisional: bool,
    ) {
        panic!("tracked field ingredients have no outputs")
    }

    fn fmt_index(
        &self,
        index: Option<crate::Id>,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(
            fmt,
            "{}.{}({:?})",
            C::DEBUG_NAME,
            C::FIELD_DEBUG_NAMES[self.field_index],
            index.unwrap()
        )
    }

    fn debug_name(&self) -> &'static str {
        C::FIELD_DEBUG_NAMES[self.field_index]
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
