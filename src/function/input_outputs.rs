use crate::{
    accumulator::accumulated_map::AccumulatedMap, zalsa::Zalsa, zalsa_local::QueryOrigin, Id,
};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn origin(&self, zalsa: &Zalsa, key: Id) -> Option<QueryOrigin> {
        self.get_memo_from_table_for(zalsa, key)
            .map(|m| m.revisions.origin.clone())
    }

    pub(super) fn accumulated(&self, zalsa: &Zalsa, key: Id) -> Option<&AccumulatedMap> {
        // NEXT STEP: stash and refactor `fetch` to return an `&Memo` so we can make this work
        self.get_memo_from_table_for(zalsa, key)
            .map(|m| &m.revisions.accumulated)
    }
}
