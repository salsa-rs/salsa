use crate::runtime::local_state::QueryEdges;

use super::{Configuration, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    pub(super) fn inputs(&self, key: C::Key) -> Option<QueryEdges> {
        self.memo_map.get(key).map(|m| m.revisions.edges.clone())
    }
}
