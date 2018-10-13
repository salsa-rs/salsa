use parking_lot::{Condvar, Mutex};
use salsa::Database;
use salsa::ParallelDatabase;
use std::cell::Cell;
use std::sync::Arc;

salsa::query_group! {
    pub(crate) trait ParDatabase: Knobs + salsa::Database {
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

/// Various "knobs" and utilities used by tests to force
/// a certain behavior.
pub(crate) trait Knobs {
    fn signal(&self) -> &Signal;

    /// Invocations of `sum` will signal `stage` this stage on entry.
    fn sum_signal_on_entry(&self) -> &Cell<usize>;

    /// If set to true, invocations of `sum` will await cancellation
    /// before they exit.
    fn sum_await_cancellation(&self) -> &Cell<bool>;
}

pub(crate) trait WithValue<T> {
    fn with_value<R>(&self, value: T, closure: impl FnOnce() -> R) -> R;
}

impl<T> WithValue<T> for Cell<T> {
    fn with_value<R>(&self, value: T, closure: impl FnOnce() -> R) -> R {
        let old_value = self.replace(value);

        let result = closure();

        self.set(old_value);

        result
    }
}

#[derive(Clone, Default)]
struct KnobsStruct {
    signal: Arc<Signal>,
    sum_signal_on_entry: Cell<usize>,
    sum_await_cancellation: Cell<bool>,
}

#[derive(Default)]
pub(crate) struct Signal {
    value: Mutex<usize>,
    cond_var: Condvar,
}

impl Signal {
    pub(crate) fn signal(&self, stage: usize) {
        log::debug!("signal({})", stage);
        let mut v = self.value.lock();
        if stage > *v {
            *v = stage;
            self.cond_var.notify_all();
        }
    }

    /// Waits until the given condition is true; the fn is invoked
    /// with the current stage.
    pub(crate) fn await(&self, stage: usize) {
        log::debug!("await({})", stage);
        let mut v = self.value.lock();
        while *v < stage {
            self.cond_var.wait(&mut v);
        }
    }
}

fn sum(db: &impl ParDatabase, key: &'static str) -> usize {
    let mut sum = 0;

    let stage = db.sum_signal_on_entry().get();
    db.signal().signal(stage);

    for ch in key.chars() {
        sum += db.input(ch);
    }

    if db.sum_await_cancellation().get() {
        log::debug!("awaiting cancellation");
        while !db.salsa_runtime().is_current_revision_canceled() {
            std::thread::yield_now();
        }
        log::debug!("cancellation observed");
        return std::usize::MAX; // when we are cancelled, we return usize::MAX.
    }

    sum
}

#[derive(Default)]
pub struct ParDatabaseImpl {
    runtime: salsa::Runtime<ParDatabaseImpl>,
    knobs: KnobsStruct,
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
            knobs: self.knobs.clone(),
        }
    }
}

impl Knobs for ParDatabaseImpl {
    fn signal(&self) -> &Signal {
        &self.knobs.signal
    }

    fn sum_signal_on_entry(&self) -> &Cell<usize> {
        &self.knobs.sum_signal_on_entry
    }

    fn sum_await_cancellation(&self) -> &Cell<bool> {
        &self.knobs.sum_await_cancellation
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
