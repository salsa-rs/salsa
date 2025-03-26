use crate::function::{Configuration, IngredientImpl};
use crate::zalsa::Zalsa;
use crate::zalsa_local::QueryOrigin;
use crate::Id;

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
