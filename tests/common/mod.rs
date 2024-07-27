//! Utility for tests that lets us log when notable events happen.

#![allow(dead_code)]

use salsa::{DatabaseImpl, UserData};

/// Logging userdata: provides [`LogDatabase`][] trait.
///
/// If you wish to use it along with other userdata,
/// you can also embed it in another struct and implement [`HasLogger`][] for that struct.
#[derive(Default)]
pub struct Logger {
    logs: std::sync::Mutex<Vec<String>>,
}

impl UserData for Logger {}

#[salsa::db]
pub trait LogDatabase: HasLogger + salsa::Database {
    /// Log an event from inside a tracked function.
    fn push_log(&self, string: String) {
        self.logger().logs.lock().unwrap().push(string);
    }

    /// Asserts what the (formatted) logs should look like,
    /// clearing the logged events. This takes `&mut self` because
    /// it is meant to be run from outside any tracked functions.
    fn assert_logs(&mut self, expected: expect_test::Expect) {
        let logs = std::mem::take(&mut *self.logger().logs.lock().unwrap());
        expected.assert_eq(&format!("{:#?}", logs));
    }

    /// Asserts the length of the logs,
    /// clearing the logged events. This takes `&mut self` because
    /// it is meant to be run from outside any tracked functions.
    fn assert_logs_len(&mut self, expected: usize) {
        let logs = std::mem::take(&mut *self.logger().logs.lock().unwrap());
        assert_eq!(logs.len(), expected);
    }
}

#[salsa::db]
impl<U: HasLogger + UserData> LogDatabase for DatabaseImpl<U> {}

/// Trait implemented by databases that lets them log events.
pub trait HasLogger {
    /// Return a reference to the logger from the database.
    fn logger(&self) -> &Logger;
}

impl<U: HasLogger + UserData> HasLogger for DatabaseImpl<U> {
    fn logger(&self) -> &Logger {
        U::logger(self)
    }
}

impl HasLogger for Logger {
    fn logger(&self) -> &Logger {
        self
    }
}

/// Userdata that provides logging and logs salsa events.
#[derive(Default)]
pub struct EventLogger {
    logger: Logger,
}

impl UserData for EventLogger {
    fn salsa_event(db: &DatabaseImpl<Self>, event: &dyn Fn() -> salsa::Event) {
        db.push_log(format!("{:?}", event()));
    }
}

impl HasLogger for EventLogger {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[derive(Default)]
pub struct DiscardLogger(Logger);

impl UserData for DiscardLogger {
    fn salsa_event(db: &DatabaseImpl<DiscardLogger>, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        match event.kind {
            salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. } => {
                db.push_log(format!("salsa_event({:?})", event.kind));
            }
            _ => {}
        }
    }
}

impl HasLogger for DiscardLogger {
    fn logger(&self) -> &Logger {
        &self.0
    }
}

#[derive(Default)]
pub struct ExecuteValidateLogger(Logger);

impl UserData for ExecuteValidateLogger {
    fn salsa_event(db: &DatabaseImpl<Self>, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        match event.kind {
            salsa::EventKind::WillExecute { .. }
            | salsa::EventKind::DidValidateMemoizedValue { .. } => {
                db.push_log(format!("salsa_event({:?})", event.kind));
            }
            _ => {}
        }
    }
}

impl HasLogger for ExecuteValidateLogger {
    fn logger(&self) -> &Logger {
        &self.0
    }
}
