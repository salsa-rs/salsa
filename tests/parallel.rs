use parking_lot::{Condvar, Mutex};
use salsa::Database;
use salsa::ParallelDatabase;
use std::cell::Cell;
use std::sync::Arc;

#[derive(Default)]
pub struct ParDatabaseImpl {
    runtime: salsa::Runtime<ParDatabaseImpl>,
    signal: Arc<Signal>,
}

impl Database for ParDatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<ParDatabaseImpl> {
        &self.runtime
    }
}

impl ParallelDatabase for ParDatabaseImpl {
    fn fork(&self) -> Self {
        ParDatabaseImpl {
            runtime: self.runtime.fork(),
            signal: self.signal.clone(),
        }
    }
}

salsa::database_storage! {
    pub struct DatabaseImplStorage for ParDatabaseImpl {
        impl ParDatabase {
            fn input() for Input;
            fn sum() for Sum;
        }
    }
}

salsa::query_group! {
    trait ParDatabase: HasSignal + salsa::Database {
        fn input(key: char) -> usize {
            type Input;
            storage input;
        }

        fn sum(key: &'static str) -> usize {
            type Sum;
            use fn sum;
        }
    }
}

// This is used to force `sum` to block on the signal sometimes so
// that we can forcibly arrange race conditions we would like to test.
thread_local! {
    static SUM_SHOULD_AWAIT_CANCELLATION: Cell<bool> = Cell::new(false);
}

trait HasSignal {
    fn signal(&self) -> &Signal;
}

impl HasSignal for ParDatabaseImpl {
    fn signal(&self) -> &Signal {
        &self.signal
    }
}

#[derive(Default)]
struct Signal {
    value: Mutex<usize>,
    cond_var: Condvar,
}

impl Signal {
    fn signal(&self, stage: usize) {
        log::debug!("signal({})", stage);
        let mut v = self.value.lock();
        assert!(
            stage > *v,
            "stage should be increasing monotonically (old={}, new={})",
            *v,
            stage
        );
        *v = stage;
        self.cond_var.notify_all();
    }

    /// Waits until the given condition is true; the fn is invoked
    /// with the current stage.
    fn await(&self, stage: usize) {
        log::debug!("await({})", stage);
        let mut v = self.value.lock();
        while *v < stage {
            self.cond_var.wait(&mut v);
        }
    }
}

fn sum(db: &impl ParDatabase, key: &'static str) -> usize {
    let mut sum = 0;

    // If we are going to await cancellation, we first *signal* when
    // we have entered. This way, the other thread can wait and be
    // sure that we are executing `sum`.
    if SUM_SHOULD_AWAIT_CANCELLATION.with(|s| s.get()) {
        db.signal().signal(1);
    }

    for ch in key.chars() {
        sum += db.input(ch);
    }

    if SUM_SHOULD_AWAIT_CANCELLATION.with(|s| s.get()) {
        log::debug!("awaiting cancellation");
        while !db.salsa_runtime().is_current_revision_canceled() {
            std::thread::yield_now();
        }
        log::debug!("cancellation observed");
        return std::usize::MAX; // when we are cancelled, we return usize::MAX.
    }

    sum
}

#[test]
fn in_par() {
    let db1 = ParDatabaseImpl::default();
    let db2 = db1.fork();

    db1.query(Input).set('a', 100);
    db1.query(Input).set('b', 010);
    db1.query(Input).set('c', 001);
    db1.query(Input).set('d', 200);
    db1.query(Input).set('e', 020);
    db1.query(Input).set('f', 002);

    let thread1 = std::thread::spawn(move || db1.sum("abc"));

    let thread2 = std::thread::spawn(move || db2.sum("def"));

    assert_eq!(thread1.join().unwrap(), 111);
    assert_eq!(thread2.join().unwrap(), 222);
}

#[test]
fn in_par_get_set_race() {
    let db1 = ParDatabaseImpl::default();
    let db2 = db1.fork();

    db1.query(Input).set('a', 100);
    db1.query(Input).set('b', 010);
    db1.query(Input).set('c', 001);

    let thread1 = std::thread::spawn(move || {
        let v = db1.sum("abc");
        v
    });

    let thread2 = std::thread::spawn(move || {
        db2.query(Input).set('a', 1000);
        db2.sum("a")
    });

    // If the 1st thread runs first, you get 111, otherwise you get
    // 1011.
    let value1 = thread1.join().unwrap();
    assert!(value1 == 111 || value1 == 1011, "illegal result {}", value1);

    assert_eq!(thread2.join().unwrap(), 1000);
}

#[test]
fn in_par_get_set_cancellation() {
    let db = ParDatabaseImpl::default();

    db.query(Input).set('a', 100);
    db.query(Input).set('b', 010);
    db.query(Input).set('c', 001);
    db.query(Input).set('d', 0);

    let thread1 = std::thread::spawn({
        let db = db.fork();
        move || {
            SUM_SHOULD_AWAIT_CANCELLATION.with(|c| c.set(true));
            let v1 = db.sum("abc");

            // check that we observed cancellation
            assert_eq!(v1, std::usize::MAX);
            SUM_SHOULD_AWAIT_CANCELLATION.with(|c| c.set(false));

            // at this point, we have observed cancellation, so let's
            // wait until the `set` is known to have occurred.
            db.signal().await(2);

            // Now when we read we should get the correct sums. Note
            // in particular that we re-compute the sum of `"abc"`
            // even though none of our inputs have changed.
            let v2 = db.sum("abc");
            (v1, v2)
        }
    });

    let thread2 = std::thread::spawn({
        let db = db.fork();
        move || {
            // Wait until we have entered `sum` in the other thread.
            db.signal().await(1);

            db.query(Input).set('d', 1000);

            // Signal that we have *set* `d`
            db.signal().signal(2);

            db.sum("d")
        }
    });

    assert_eq!(thread1.join().unwrap(), (std::usize::MAX, 111));
    assert_eq!(thread2.join().unwrap(), 1000);
}
