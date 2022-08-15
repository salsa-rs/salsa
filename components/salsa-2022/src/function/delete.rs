use crate::runtime::local_state::QueryOrigin;

use super::{Configuration, FunctionIngredient};

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    /// Removes the memoized value for `key` from the memo-map.
    /// Pushes the memo onto `deleted_entries` to ensure that any references into that memo which were handed out remain valid.
    pub(super) fn delete_memo(&self, key: C::Key) -> Option<QueryOrigin> {
        if let Some(memo) = self.memo_map.remove(key) {
            let origin = memo.load().revisions.origin.clone();
            self.deleted_entries.push(memo);
            Some(origin)
        } else {
            None
        }
    }
}
