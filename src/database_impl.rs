use crate::storage::HasStorage;
use crate::{Database, Event, Storage};

/// Default database implementation that you can use if you don't
/// require any custom user data.
#[derive(Default, Clone)]
pub struct DatabaseImpl {
    storage: Storage<Self>,
}

impl DatabaseImpl {
    /// Create a new database; equivalent to `Self::default`.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn storage(&self) -> &Storage<Self> {
        &self.storage
    }
}

impl Database for DatabaseImpl {
    /// Default behavior: tracing debug log the event.
    fn salsa_event(&self, event: &dyn Fn() -> Event) {
        tracing::debug!("salsa_event({:?})", event());
    }
}

// SAFETY: The `storage` and `storage_mut` fields return a reference to the same storage field owned by `self`.
unsafe impl HasStorage for DatabaseImpl {
    fn storage(&self) -> &Storage<Self> {
        &self.storage
    }

    fn storage_mut(&mut self) -> &mut Storage<Self> {
        &mut self.storage
    }
}
