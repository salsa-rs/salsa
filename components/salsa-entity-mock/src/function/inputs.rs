use crate::runtime::local_state::QueryInputs;

use super::{Configuration, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    pub(super) fn inputs(&self, key: C::Key) -> Option<QueryInputs> {
        self.memo_map.get(key).map(|m| m.revisions.inputs.clone())
    }
}
