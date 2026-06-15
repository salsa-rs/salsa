use std::ptr::NonNull;

use crate::function::Configuration;
use crate::function::memo::Memo;

/// Stores memos that must remain alive until the next revision.
pub(super) struct DeletedEntries<C: Configuration> {
    memos: boxcar::Vec<SharedBox<Memo<'static, C>>>,
}

#[allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety
unsafe impl<T: Send> Send for SharedBox<T> {}
#[allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety
unsafe impl<T: Sync> Sync for SharedBox<T> {}

impl<C: Configuration> Default for DeletedEntries<C> {
    fn default() -> Self {
        Self {
            memos: Default::default(),
        }
    }
}

impl<C: Configuration> DeletedEntries<C> {
    /// # Safety
    ///
    /// The memo must be valid and safe to free when this list is cleared or dropped.
    pub(super) unsafe fn push(&self, memo: NonNull<Memo<'_, C>>) {
        // SAFETY: The memo is kept alive until the next revision.
        let memo =
            unsafe { std::mem::transmute::<NonNull<Memo<'_, C>>, NonNull<Memo<'static, C>>>(memo) };

        self.memos.push(SharedBox(memo));
    }

    /// Defers freeing a retired volatile memo until active epoch readers have exited.
    ///
    /// # Safety
    ///
    /// The memo must have been removed from the memo table.
    pub(super) unsafe fn push_retired(&self, memo: NonNull<Memo<'_, C>>) {
        // SAFETY: The allocation remains valid until the deferred callback runs.
        let memo =
            unsafe { std::mem::transmute::<NonNull<Memo<'_, C>>, NonNull<Memo<'static, C>>>(memo) };
        let memo = SharedBox(memo);

        crossbeam_epoch::pin().defer(move || drop(memo));
    }

    /// Free all revision-delayed memos, keeping the list available for reuse.
    pub(super) fn clear(&mut self) {
        self.memos.clear();
    }
}

/// A wrapper around `NonNull` that frees the allocation when it is dropped.
struct SharedBox<T>(NonNull<T>);

impl<T> Drop for SharedBox<T> {
    fn drop(&mut self) {
        // SAFETY: Guaranteed by the caller of `DeletedEntries::push` or `push_retired`.
        unsafe { drop(Box::from_raw(self.0.as_ptr())) }
    }
}
