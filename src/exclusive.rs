/// `std::sync::Exclusive`
pub struct Exclusive<T: ?Sized> {
    inner: T,
}

unsafe impl<T> Sync for Exclusive<T> {}

impl<T> Exclusive<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
