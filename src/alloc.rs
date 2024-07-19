use std::ptr::NonNull;

/// A box but without the uniqueness guarantees.
pub struct Alloc<T> {
    data: NonNull<T>,
}

impl<T> Alloc<T> {
    pub fn new(data: T) -> Self {
        let data = Box::new(data);
        let data = Box::into_raw(data);
        Alloc {
            data: unsafe { NonNull::new_unchecked(data) },
        }
    }

    pub fn as_raw(&self) -> NonNull<T> {
        self.data
    }

    pub unsafe fn as_ref(&self) -> &T {
        unsafe { self.data.as_ref() }
    }

    pub unsafe fn as_mut(&mut self) -> &mut T {
        unsafe { self.data.as_mut() }
    }
}

impl<T> Drop for Alloc<T> {
    fn drop(&mut self) {
        let data: *mut T = self.data.as_ptr();
        let data: Box<T> = unsafe { Box::from_raw(data) };
        drop(data);
    }
}

unsafe impl<T> Send for Alloc<T> where T: Send {}

unsafe impl<T> Sync for Alloc<T> where T: Sync {}
