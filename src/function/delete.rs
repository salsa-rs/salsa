use crossbeam::queue::SegQueue;

use super::{memo::ArcMemo, Configuration};

/// Stores the list of memos that have been deleted so they can be freed
/// once the next revision starts. See the comment on the field
/// `deleted_entries` of [`FunctionIngredient`][] for more details.
pub(super) struct DeletedEntries<C: Configuration> {
    seg_queue: SegQueue<ArcMemo<'static, C>>,
}

impl<C: Configuration> Default for DeletedEntries<C> {
    fn default() -> Self {
        Self {
            seg_queue: Default::default(),
        }
    }
}

impl<C: Configuration> DeletedEntries<C> {
    pub(super) fn push<'db>(&'db self, memo: ArcMemo<'db, C>) {
        let memo = unsafe { std::mem::transmute::<ArcMemo<'db, C>, ArcMemo<'static, C>>(memo) };
        self.seg_queue.push(memo);
    }
}
