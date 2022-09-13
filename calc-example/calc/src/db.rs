use std::sync::{Arc, Mutex};

use salsa::DebugWithDb;

use crate::PushLog;

// ANCHOR: db_struct
#[derive(Default)]
#[salsa::db(crate::Jar)]
pub(crate) struct Database {
    storage: salsa::Storage<Self>,

    /// When compiled in `cfg(test)` mode, this field stores a shared vector
    /// that we can use to accumulate logs for testing. The `push_log` method
    /// from the [`PushLog`](`crate::PushLog`) trait adds to the logs,
    /// and the `take_logs` method clears out the existing contents.
    /// When not in `cfg(test)` mode, the field is `None`.
    ///
    /// NB: This demonstrates how you can add additional state to your database.
    /// Be aware that, if you want to support parallel execution, each thread will
    /// get their own handle to the database, so you either need to be able to
    /// clone/share the state or else to give each thread its own copy.
    logs: Option<Arc<Mutex<Vec<String>>>>,
}
// ANCHOR_END: db_struct

// ANCHOR: LoggingSupportCode
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
// ANCHOR_END: LoggingSupportCode

// ANCHOR: PushLogImpl
impl PushLog for Database {
    fn push_log(&self, message: &mut dyn FnMut() -> String) {
        if let Some(logs) = &self.logs {
            logs.lock().unwrap().push(message());
        }
    }
}
// ANCHOR_END: PushLogImpl

// ANCHOR: db_impl
impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        log::debug!("salsa_event: {:?}", event.debug(self));

        // Log the event for when functions are being executed,
        // so that our tests can observe it to see how much reuse we are getting.
        if let salsa::EventKind::WillExecute { .. } = event.kind {
            self.push_log(&mut || format!("Event: {:?}", event.debug(self)));
        }
    }
}
// ANCHOR_END: db_impl

// ANCHOR: par_db_impl
impl salsa::ParallelDatabase for Database {
    /// The snapshot method creates a second database handle
    /// and wraps it in `Snapshot`. This new handle can be sent
    /// to another thread. The `Snapshot` wrapper owns the database
    /// and permits only `&` access to its contents, so that this other
    /// thread cannot mutate inputs, as that would require `&mut` access.
    fn snapshot(&self) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Database {
            storage: self.storage.snapshot(),
            logs: self.logs.clone(),
        })
    }
}
// ANCHOR_END: par_db_impl
