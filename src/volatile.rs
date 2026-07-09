use std::ops::Deref;
use std::sync::Arc;

/// An owned handle to a volatile query result.
///
/// Eviction removes the result from Salsa's memo immediately, while existing
/// handles keep the value alive until their last clone is dropped.
#[derive(Debug, PartialEq, Eq)]
pub struct Volatile<T>(pub(crate) Arc<T>);

impl<T> Clone for Volatile<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Volatile<T> {
    /// Converts this handle into the shared value it owns.
    pub fn into_arc(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Volatile<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
