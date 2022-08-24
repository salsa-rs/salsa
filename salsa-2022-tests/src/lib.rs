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
