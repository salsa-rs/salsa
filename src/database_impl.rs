use crate::{self as salsa, Database, Event, Storage};

#[salsa::db]
/// Default database implementation that you can use if you don't
/// require any custom user data.
#[derive(Default)]
pub struct DatabaseImpl {
    storage: Storage<Self>,
}

impl DatabaseImpl {
    /// Create a new database; equivalent to `Self::default`.
    pub fn new() -> Self {
        Self::default()
    }
}

#[salsa::db]
impl Database for DatabaseImpl {
    /// Default behavior: tracing debug log the event.
    fn salsa_event(&self, event: &dyn Fn() -> Event) {
        tracing::debug!("salsa_event({:?})", event());
    }
}
