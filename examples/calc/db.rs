use std::sync::{Arc, Mutex};

use salsa::UserData;

pub type CalcDatabaseImpl = salsa::DatabaseImpl<Calc>;

// ANCHOR: db_struct
#[derive(Default)]
pub struct Calc {
    // The logs are only used for testing and demonstrating reuse:
    logs: Arc<Mutex<Option<Vec<String>>>>,
}
// ANCHOR_END: db_struct

impl Calc {
    /// Enable logging of each salsa event.
    #[cfg(test)]
    pub fn enable_logging(&self) {
        let mut logs = self.logs.lock().unwrap();
        if logs.is_none() {
            *logs = Some(vec![]);
        }
    }

    #[cfg(test)]
    pub fn take_logs(&self) -> Vec<String> {
        let mut logs = self.logs.lock().unwrap();
        if let Some(logs) = &mut *logs {
            std::mem::take(logs)
        } else {
            vec![]
        }
    }
}

// ANCHOR: db_impl
impl UserData for Calc {
    fn salsa_event(db: &CalcDatabaseImpl, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        eprintln!("Event: {event:?}");
        // Log interesting events, if logging is enabled
        if let Some(logs) = &mut *db.logs.lock().unwrap() {
            // only log interesting events
            if let salsa::EventKind::WillExecute { .. } = event.kind {
                logs.push(format!("Event: {event:?}"));
            }
        }
    }
}
// ANCHOR_END: db_impl
