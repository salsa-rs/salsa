use crate::{zalsa_local::QueryOrigin, Id};

use super::{Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn origin(&self, key: Id) -> Option<QueryOrigin> {
        self.memo_map.get(key).map(|m| m.revisions.origin.clone())
    }
}
