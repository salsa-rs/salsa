use std::ptr::NonNull;

use crossbeam_queue::SegQueue;

use super::memo::Memo;
use super::Configuration;

/// Stores the list of memos that have been deleted so they can be freed
/// once the next revision starts. See the comment on the field
/// `deleted_entries` of [`FunctionIngredient`][] for more details.
pub(super) struct DeletedEntries<C: Configuration> {
    seg_queue: SegQueue<SharedBox<Memo<C::Output<'static>>>>,
}

unsafe impl<T: Send> Send for SharedBox<T> {}
unsafe impl<T: Sync> Sync for SharedBox<T> {}

impl<C: Configuration> Default for DeletedEntries<C> {
    fn default() -> Self {
        Self {
            seg_queue: Default::default(),
        }
    }
}

impl<C: Configuration> DeletedEntries<C> {
    /// # Safety
    ///
    /// The memo must be valid and safe to free when the `DeletedEntries` list is dropped.
    pub(super) unsafe fn push(&self, memo: NonNull<Memo<C::Output<'_>>>) {
        let memo = unsafe {
            std::mem::transmute::<NonNull<Memo<C::Output<'_>>>, NonNull<Memo<C::Output<'static>>>>(
                memo,
            )
        };

        self.seg_queue.push(SharedBox(memo));
    }
}

/// A wrapper around `NonNull` that frees the allocation when it is dropped.
///
/// `crossbeam::SegQueue` does not expose mutable accessors so we have to create
/// a wrapper to run code during `Drop`.
struct SharedBox<T>(NonNull<T>);

impl<T> Drop for SharedBox<T> {
    fn drop(&mut self) {
        // SAFETY: Guaranteed by the caller of `DeletedEntries::push`.
        unsafe { drop(Box::from_raw(self.0.as_ptr())) };
    }
}
