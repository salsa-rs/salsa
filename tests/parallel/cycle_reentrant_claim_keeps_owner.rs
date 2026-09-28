//! Releasing a reentrant claim must preserve waits already aimed at its owner.
//!
//! D reads Q and C, Q reads D, and C reads Q. Q transfers its claim to D,
//! then C's thread waits for Q on D's thread. D selects C as its outer cycle
//! head before its recovery callback rereads Q. That read reentrantly claims Q
//! on D's thread, which already owns Q's transferred lock. Waking C when this claim
//! ends would invalidate the dependency that D's pending transfer to C needs.

use salsa::{Database, Storage};

use crate::sync::{Arc, Condvar, Mutex, thread};

// Unlike the shared Knobs signals, these stages also coordinate Shuttle runs.
#[derive(Default)]
struct Signal {
    stage: Mutex<usize>,
    ready: Condvar,
}

impl Signal {
    fn signal(&self, stage: usize) {
        let mut current = self.stage.lock().unwrap();
        if *current < stage {
            *current = stage;
            self.ready.notify_all();
        }
    }

    fn wait(&self, stage: usize) {
        let mut current = self.stage.lock().unwrap();
        while *current < stage {
            current = self.ready.wait(current).unwrap();
        }
    }
}

#[salsa::db]
trait Db: Database {
    fn signal(&self) -> &Signal;
}

#[salsa::db]
#[derive(Clone)]
struct TestDb {
    storage: Storage<Self>,
    signal: Arc<Signal>,
}

#[salsa::db]
impl Database for TestDb {}

#[salsa::db]
impl Db for TestDb {
    fn signal(&self) -> &Signal {
        &self.signal
    }
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0, cycle_fn = recover_d)]
fn d(db: &dyn Db) -> u32 {
    let value = q(db);
    db.signal().signal(1);
    db.signal().wait(2);
    value | c(db) | 1
}

fn recover_d(db: &dyn Db, _cycle: &salsa::Cycle, _last: &u32, new: u32) -> u32 {
    let _ = q(db);
    new
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn q(db: &dyn Db) -> u32 {
    d(db) | 2
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn c(db: &dyn Db) -> u32 {
    q(db) | 4
}

#[test_log::test]
fn reentrant_read_preserves_existing_owner_waits() {
    crate::sync::check(|| {
        let signal = Arc::<Signal>::default();
        let event_signal = signal.clone();
        let db = TestDb {
            storage: Storage::new(Some(Box::new(move |event| {
                if matches!(event.kind, salsa::EventKind::WillBlockOn { .. }) {
                    event_signal.signal(2);
                }
            }))),
            signal,
        };

        let db_d = db.clone();
        let reader_d = thread::spawn(move || d(&db_d));

        // Start C after Q has transferred to D, then let D proceed once C blocks.
        db.signal().wait(1);
        let db_c = db.clone();
        let reader_c = thread::spawn(move || c(&db_c));

        assert_eq!(reader_d.join().unwrap(), 7);
        assert_eq!(reader_c.join().unwrap(), 7);
    });
}
