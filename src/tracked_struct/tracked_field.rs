use std::marker::PhantomData;

use crate::cycle::CycleHeads;
use crate::function::VerifyResult;
use crate::ingredient::Ingredient;
use crate::sync::Arc;
use crate::table::memo::MemoTableTypes;
use crate::tracked_struct::{Configuration, Value};
use crate::zalsa::IngredientIndex;
use crate::{Database, Id};

/// Created for each tracked struct.
///
/// This ingredient only stores the "id" fields.
/// It is a kind of "dressed up" interner;
/// the active query + values of id fields are hashed to create the tracked struct id.
/// The value fields are stored in [`crate::function::IngredientImpl`] instances keyed by the tracked struct id.
/// Unlike normal interners, tracked struct indices can be deleted and reused aggressively:
/// when a tracked function re-executes,
/// any tracked structs that it created before but did not create this time can be deleted.
pub struct FieldIngredientImpl<C>
where
    C: Configuration,
{
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,

    /// The index of this field on the tracked struct relative to all other tracked fields.
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
    fn location(&self) -> &'static crate::ingredient::Location {
        &C::LOCATION
    }

    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    unsafe fn maybe_changed_after<'db>(
        &'db self,
        db: &'db dyn Database,
        input: Id,
        revision: crate::Revision,
        _cycle_heads: &mut CycleHeads,
    ) -> VerifyResult {
        let data = <super::IngredientImpl<C>>::data_raw(db.zalsa().table(), input);
        let field_changed_at = unsafe { (&(*data).revisions)[self.field_index] };
        VerifyResult::changed_if(field_changed_at > revision)
    }

    fn fmt_index(&self, index: crate::Id, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "{}.{}({:?})",
            C::DEBUG_NAME,
            C::TRACKED_FIELD_NAMES[self.field_index],
            index
        )
    }

    fn debug_name(&self) -> &'static str {
        C::TRACKED_FIELD_NAMES[self.field_index]
    }

    fn memo_table_types(&self) -> Arc<MemoTableTypes> {
        unreachable!("tracked field does not allocate pages")
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
