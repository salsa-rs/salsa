use std::{alloc, ptr};

use crate::storage::HasJars;

/// Initializes the `DB`'s jars in-place
///
/// # Safety:
///
/// `init` must fully initialize all of jars fields
pub unsafe fn create_jars_inplace<DB: HasJars>(init: impl FnOnce(*mut DB::Jars)) -> Box<DB::Jars> {
    let layout = alloc::Layout::new::<DB::Jars>();

    if layout.size() == 0 {
        // SAFETY: This is the recommended way of creating a Box
        // to a ZST in the std docs
        unsafe { Box::from_raw(ptr::NonNull::dangling().as_ptr()) }
    } else {
        // SAFETY: We've checked that the size isn't 0
        let place = unsafe { alloc::alloc_zeroed(layout) };
        let place = place.cast::<DB::Jars>();

        init(place);

        // SAFETY: Caller invariant requires that `init` must've
        // initialized all of the fields
        unsafe { Box::from_raw(place) }
    }
}
