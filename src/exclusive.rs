//! Bare-bones polyfill for the unstable [`std::sync::Exclusive`] type.

pub struct Exclusive<T: ?Sized> {
    inner: T,
}

// SAFETY: We only hand out mutable access to the inner value through a mutable reference to the
// wrapper.
// Therefore we cannot alias the inner value making it trivially sync.
unsafe impl<T> Sync for Exclusive<T> {}

impl<T> Exclusive<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
