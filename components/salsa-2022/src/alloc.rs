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
}

impl<T> Drop for Alloc<T> {
    fn drop(&mut self) {
        let data: *mut T = self.data.as_ptr();
        let data: Box<T> = unsafe { Box::from_raw(data) };
        drop(data);
    }
}

impl<T> std::ops::Deref for Alloc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.data.as_ref() }
    }
}

impl<T> std::ops::DerefMut for Alloc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.data.as_mut() }
    }
}

unsafe impl<T> Send for Alloc<T> where T: Send {}

unsafe impl<T> Sync for Alloc<T> where T: Sync {}
