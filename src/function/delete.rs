use arc_swap::ArcSwap;
use crossbeam::queue::SegQueue;

use crate::{zalsa_local::QueryOrigin, Id};

use super::{memo, Configuration, IngredientImpl};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Removes the memoized value for `key` from the memo-map.
    /// Pushes the memo onto `deleted_entries` to ensure that any references into that memo which were handed out remain valid.
    pub(super) fn delete_memo(&self, key: Id) -> Option<QueryOrigin> {
        if let Some(memo) = self.memo_map.remove(key) {
            let origin = memo.load().revisions.origin.clone();
            self.deleted_entries.push(memo);
            Some(origin)
        } else {
            None
        }
    }
}

/// Stores the list of memos that have been deleted so they can be freed
/// once the next revision starts. See the comment on the field
/// `deleted_entries` of [`FunctionIngredient`][] for more details.
pub(super) struct DeletedEntries<C: Configuration> {
    seg_queue: SegQueue<ArcSwap<memo::Memo<C::Output<'static>>>>,
}

impl<C: Configuration> Default for DeletedEntries<C> {
    fn default() -> Self {
        Self {
            seg_queue: Default::default(),
        }
    }
}

impl<C: Configuration> DeletedEntries<C> {
    pub(super) fn push<'db>(&'db self, memo: ArcSwap<memo::Memo<C::Output<'db>>>) {
        let memo = unsafe {
            std::mem::transmute::<
                ArcSwap<memo::Memo<C::Output<'db>>>,
                ArcSwap<memo::Memo<C::Output<'static>>>,
            >(memo)
        };
        self.seg_queue.push(memo);
    }
}
