use std::sync::{Arc, Mutex};

use salsa::DebugWithDb;

// ANCHOR: db_struct
#[derive(Default)]
#[salsa::db(crate::Jar)]
pub(crate) struct Database {
    storage: salsa::Storage<Self>,

    // The logs are only used for testing and demonstrating reuse:
    //
    logs: Option<Arc<Mutex<Vec<String>>>>,
}
// ANCHOR_END: db_struct

impl Database {
    /// Enable logging of each salsa event.
    #[cfg(test)]
    pub fn enable_logging(self) -> Self {
        assert!(self.logs.is_none());
        Self {
            storage: self.storage,
            logs: Some(Default::default()),
        }
    }

    #[cfg(test)]
    pub fn take_logs(&mut self) -> Vec<String> {
        if let Some(logs) = &self.logs {
            std::mem::take(&mut *logs.lock().unwrap())
        } else {
            panic!("logs not enabled");
        }
    }
}

// ANCHOR: db_impl
impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        // Log interesting events, if logging is enabled
        if let Some(logs) = &self.logs {
            // don't log boring events
            if let salsa::EventKind::WillExecute { .. } = event.kind {
                logs.lock()
                    .unwrap()
                    .push(format!("Event: {:?}", event.debug(self)));
            }
        }
    }
}
// ANCHOR_END: db_impl

// ANCHOR: par_db_impl
impl salsa::ParallelDatabase for Database {
    fn snapshot(&self) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Database {
            storage: self.storage.snapshot(),
            logs: self.logs.clone(),
        })
    }
}
// ANCHOR_END: par_db_impl
