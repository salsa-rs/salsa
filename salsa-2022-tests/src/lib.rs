/// Utility for tests that lets us log when notable events happen.
#[derive(Default)]
pub struct Logger {
    logs: std::sync::Mutex<Vec<String>>,
}

/// Trait implemented by databases that lets them log events.
pub trait HasLogger {
    /// Return a reference to the logger from the database.
    fn logger(&self) -> &Logger;

    /// Log an event from inside a tracked function.
    fn push_log(&self, string: String) {
        self.logger().logs.lock().unwrap().push(string);
    }

    /// Asserts what the (formatted) logs should look like,
    /// clearing the logged events. This takes `&mut self` because
    /// it is meant to be run from outside any tracked functions.
    fn assert_logs(&mut self, expected: expect_test::Expect) {
        let logs = std::mem::replace(&mut *self.logger().logs.lock().unwrap(), vec![]);
        expected.assert_eq(&format!("{:#?}", logs));
    }
}
