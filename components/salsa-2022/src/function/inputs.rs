use crate::runtime::local_state::QueryOrigin;

use super::{Configuration, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    pub(super) fn origin(&self, key: C::Key) -> Option<QueryOrigin> {
        self.memo_map.get(key).map(|m| m.revisions.origin.clone())
    }
}
