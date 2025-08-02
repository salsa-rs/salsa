use std::fmt;
use std::marker::PhantomData;

use crate::cycle::CycleHeadKeys;
use crate::function::VerifyResult;
use crate::ingredient::Ingredient;
use crate::input::{Configuration, IngredientImpl, Value};
use crate::sync::Arc;
use crate::table::memo::MemoTableTypes;
use crate::zalsa::IngredientIndex;
use crate::{Id, Revision};

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
    phantom: PhantomData<fn() -> Value<C>>,
}

impl<C> FieldIngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn new(struct_index: IngredientIndex, field_index: usize) -> Self {
        Self {
            index: struct_index.successor(field_index),
            field_index,
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
        self.index
    }

    unsafe fn maybe_changed_after(
        &self,
        zalsa: &crate::zalsa::Zalsa,
        _db: crate::database::RawDatabase<'_>,
        input: Id,
        revision: Revision,
        _cycle_heads: &mut CycleHeadKeys,
    ) -> VerifyResult {
        let value = <IngredientImpl<C>>::data(zalsa, input);
        VerifyResult::changed_if(value.revisions[self.field_index] > revision)
    }

    fn fmt_index(&self, index: crate::Id, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "{input}.{field}({id:?})",
            input = C::DEBUG_NAME,
            field = C::FIELD_DEBUG_NAMES[self.field_index],
            id = index
        )
    }

    fn debug_name(&self) -> &'static str {
        C::FIELD_DEBUG_NAMES[self.field_index]
    }

    fn memo_table_types(&self) -> &Arc<MemoTableTypes> {
        unreachable!("input fields do not allocate pages")
    }

    fn memo_table_types_mut(&mut self) -> &mut Arc<MemoTableTypes> {
        unreachable!("input fields do not allocate pages")
    }
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
