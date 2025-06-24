use std::ptr::NonNull;

use crate::function::memo::Memo;
use crate::function::Configuration;

/// Stores the list of memos that have been deleted so they can be freed
/// once the next revision starts. See the comment on the field
/// `deleted_entries` of [`FunctionIngredient`][] for more details.
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
    /// The memo must be valid and safe to free when the `DeletedEntries` list is cleared or dropped.
    pub(super) unsafe fn push(&self, memo: NonNull<Memo<'_, C>>) {
        // Safety: The memo must be valid and safe to free when the `DeletedEntries` list is cleared or dropped.
        let memo =
            unsafe { std::mem::transmute::<NonNull<Memo<'_, C>>, NonNull<Memo<'static, C>>>(memo) };

        self.memos.push(SharedBox(memo));
    }

    /// Free all deleted memos, keeping the list available for reuse.
    pub(super) fn clear(&mut self) {
        self.memos.clear();
    }
}

/// A wrapper around `NonNull` that frees the allocation when it is dropped.
struct SharedBox<T>(NonNull<T>);

impl<T> Drop for SharedBox<T> {
    fn drop(&mut self) {
        // SAFETY: Guaranteed by the caller of `DeletedEntries::push`.
        unsafe { drop(Box::from_raw(self.0.as_ptr())) };
    }
}
