//! Reexecuting A must repair readers' waits even when A keeps the same owner B.
//!
//! B initially reads itself and A; A reads B. Once B has a nonzero provisional
//! result, it also reads D. D reads A and C, and C reads A. All four queries
//! therefore belong to one cycle, whose final result contains all four bits.
//!
//! D's thread reexecutes A while B's thread waits for D. C's thread then reads A
//! and blocks on D's reentrant claim. When A transfers back to B, that wait
//! must be repaired even though A's ownership transfer is unchanged. Otherwise D
//! can return an unfinished result while B and C remain blocked.

use salsa::{Database, Storage};

use crate::sync::{Arc, Condvar, Mutex, thread};

#[test_log::test]
fn transferring_to_same_owner_repairs_waiters() {
    let test = || {
        let signal = Arc::<Signal>::default();
        let event_signal = signal.clone();
        let db = TestDb {
            storage: Storage::new(Some(Box::new(move |event| {
                if matches!(event.kind, salsa::EventKind::WillBlockOn { .. }) {
                    match event_signal.current() {
                        1 => event_signal.signal(2), // B has registered its wait for D.
                        3 => event_signal.signal(4), // C has registered its wait for A.
                        _ => {}
                    }
                }
            }))),
            signal,
        };

        let db_d = db.clone();
        let reader_d = thread::spawn(move || d(&db_d));

        // Claim D before B's second iteration tries to read it.
        db.signal().wait(1);
        let db_b = db.clone();
        let reader_b = thread::spawn(move || b(&db_b));

        // Let C arrive while D's thread holds a reentrant claim on A.
        db.signal().wait(3);
        let db_c = db.clone();
        let reader_c = thread::spawn(move || c(&db_c));

        assert_eq!(reader_d.join().unwrap(), 15);
        assert_eq!(reader_b.join().unwrap(), 15);
        assert_eq!(reader_c.join().unwrap(), 15);
    };

    #[cfg(not(feature = "shuttle"))]
    test();

    #[cfg(feature = "shuttle")]
    {
        let scheduler = shuttle::scheduler::PctScheduler::new_from_seed(0, 50, 2500);
        let mut config = shuttle::Config::default();
        config.stack_size = 1024 * 1024;
        shuttle::Runner::new(scheduler, config).run(test);
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

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn a(db: &dyn Db) -> u32 {
    let value = b(db);
    if value != 0 && db.signal().current() < 4 {
        db.signal().signal(3);
        db.signal().wait(4);
    }
    value | 1
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn b(db: &dyn Db) -> u32 {
    let previous = b(db);
    if previous != 0 {
        previous | d(db) | 2
    } else {
        a(db) | 2
    }
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn d(db: &dyn Db) -> u32 {
    db.signal().signal(1);
    db.signal().wait(2);
    a(db) | c(db) | 4
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn c(db: &dyn Db) -> u32 {
    a(db) | 8
}

// These stages coordinate the required handoffs in both native and Shuttle runs.
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

    fn current(&self) -> usize {
        *self.stage.lock().unwrap()
    }
}
