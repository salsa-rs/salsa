use crate::{zalsa::Zalsa, zalsa_local::QueryOrigin, Id};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn origin<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        key: Id,
        map_key: &C::MapKey<'db>,
    ) -> Option<QueryOrigin> {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, key);
        self.get_memo_from_table_for(zalsa, key, map_key, memo_ingredient_index)
            .map(|m| m.revisions.origin.clone())
    }
}
