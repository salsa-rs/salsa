//! Utility for tests that lets us log when notable events happen.

#![allow(dead_code)]

use salsa::{Database, Storage};

/// Logging userdata: provides [`LogDatabase`][] trait.
///
/// If you wish to use it along with other userdata,
/// you can also embed it in another struct and implement [`HasLogger`][] for that struct.
#[derive(Default)]
pub struct Logger {
    logs: std::sync::Mutex<Vec<String>>,
}

/// Trait implemented by databases that lets them log events.
pub trait HasLogger {
    /// Return a reference to the logger from the database.
    fn logger(&self) -> &Logger;
}

#[salsa::db]
pub trait LogDatabase: HasLogger + Database {
    /// Log an event from inside a tracked function.
    fn push_log(&self, string: String) {
        self.logger().logs.lock().unwrap().push(string);
    }

    /// Asserts what the (formatted) logs should look like,
    /// clearing the logged events. This takes `&mut self` because
    /// it is meant to be run from outside any tracked functions.
    fn assert_logs(&self, expected: expect_test::Expect) {
        let logs = std::mem::take(&mut *self.logger().logs.lock().unwrap());
        expected.assert_eq(&format!("{:#?}", logs));
    }

    /// Asserts the length of the logs,
    /// clearing the logged events. This takes `&mut self` because
    /// it is meant to be run from outside any tracked functions.
    fn assert_logs_len(&self, expected: usize) {
        let logs = std::mem::take(&mut *self.logger().logs.lock().unwrap());
        assert_eq!(logs.len(), expected);
    }
}

#[salsa::db]
impl<Db: HasLogger + Database> LogDatabase for Db {}

/// Database that provides logging but does not log salsa event.
#[salsa::db]
#[derive(Default)]
pub struct LoggerDatabase {
    storage: Storage<Self>,
    logger: Logger,
}

impl HasLogger for LoggerDatabase {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::db]
impl Database for LoggerDatabase {
    fn salsa_event(&self, _event: &dyn Fn() -> salsa::Event) {}
}

/// Database that provides logging and logs salsa events.
#[salsa::db]
#[derive(Default)]
pub struct EventLoggerDatabase {
    storage: Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl Database for EventLoggerDatabase {
    fn salsa_event(&self, event: &dyn Fn() -> salsa::Event) {
        self.push_log(format!("{:?}", event()));
    }
}

impl HasLogger for EventLoggerDatabase {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::db]
#[derive(Default)]
pub struct DiscardLoggerDatabase {
    storage: Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl Database for DiscardLoggerDatabase {
    fn salsa_event(&self, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        match event.kind {
            salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. } => {
                self.push_log(format!("salsa_event({:?})", event.kind));
            }
            _ => {}
        }
    }
}

impl HasLogger for DiscardLoggerDatabase {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::db]
#[derive(Default)]
pub struct ExecuteValidateLoggerDatabase {
    storage: Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl Database for ExecuteValidateLoggerDatabase {
    fn salsa_event(&self, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        match event.kind {
            salsa::EventKind::WillExecute { .. }
            | salsa::EventKind::DidValidateMemoizedValue { .. } => {
                self.push_log(format!("salsa_event({:?})", event.kind));
            }
            _ => {}
        }
    }
}

impl HasLogger for ExecuteValidateLoggerDatabase {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}
