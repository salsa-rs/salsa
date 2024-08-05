use std::fmt::Debug;
use std::ops::{Deref, DerefMut};

#[derive(Copy, Clone, Debug)]
pub struct Array<T, const N: usize> {
    data: [T; N],
}

impl<T, const N: usize> Array<T, N> {
    pub fn new(data: [T; N]) -> Self {
        Self { data }
    }
}

impl<T, const N: usize> Deref for Array<T, N> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T, const N: usize> DerefMut for Array<T, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}
