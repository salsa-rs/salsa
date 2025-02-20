use crate::{zalsa::Zalsa, zalsa_local::QueryOrigin, Id};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn origin(&self, zalsa: &Zalsa, key: Id) -> Option<QueryOrigin> {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);
        self.get_memo_from_table_for(zalsa, key, memo_ingredient_index)
            .map(|m| m.revisions.origin.clone())
    }
}
