use crate::function::EvictionPolicy;
use crate::function::{Configuration, IngredientImpl};
use crate::zalsa::Zalsa;
use crate::{DatabaseKeyIndex, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn extend_origin_inputs(
        &self,
        zalsa: &Zalsa,
        key: Id,
        inputs: &mut Vec<DatabaseKeyIndex>,
    ) {
        let _guard = C::Eviction::RETIRES_VALUES.then(|| self.memo_read_guard());
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);
        let Some(memo) = self.get_memo_from_table_for(zalsa, key, memo_ingredient_index) else {
            return;
        };
        let origin = memo.header.origin();
        inputs.reserve(origin.edges().iter().len());
        inputs.extend(origin.inputs().rev());
    }
}
