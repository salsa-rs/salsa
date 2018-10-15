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
    fn knobs(&self) -> &KnobsStruct;

    fn signal(&self, stage: usize);

    fn await(&self, stage: usize);
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

/// Various "knobs" that can be used to customize how the queries
/// behave on one specific thread. Note that this state is
/// intentionally thread-local (apart from `signal`).
#[derive(Clone, Default)]
pub(crate) struct KnobsStruct {
    /// A kind of flexible barrier used to coordinate execution across
    /// threads to ensure we reach various weird states.
    pub(crate) signal: Arc<Signal>,

    /// Invocations of `sum` will signal this stage on entry.
    pub(crate) sum_signal_on_entry: Cell<usize>,

    /// Invocations of `sum` will await this stage on entry.
    pub(crate) sum_await_on_entry: Cell<usize>,

    /// If true, invocations of `sum` will await cancellation before
    /// they exit.
    pub(crate) sum_await_cancellation: Cell<bool>,

    /// Invocations of `sum` will await this stage prior to exiting.
    pub(crate) sum_await_on_exit: Cell<usize>,

    /// Invocations of `sum` will signal this stage prior to exiting.
    pub(crate) sum_signal_on_exit: Cell<usize>,
}

#[derive(Default)]
pub(crate) struct Signal {
    value: Mutex<usize>,
    cond_var: Condvar,
}

impl Signal {
    pub(crate) fn signal(&self, stage: usize) {
        log::debug!("signal({})", stage);

        // This check avoids acquiring the lock for things that will
        // clearly be a no-op. Not *necessary* but helps to ensure we
        // are more likely to encounter weird race conditions;
        // otherwise calls to `sum` will tend to be unnecessarily
        // synchronous.
        if stage > 0 {
            let mut v = self.value.lock();
            if stage > *v {
                *v = stage;
                self.cond_var.notify_all();
            }
        }
    }

    /// Waits until the given condition is true; the fn is invoked
    /// with the current stage.
    pub(crate) fn await(&self, stage: usize) {
        log::debug!("await({})", stage);

        // As above, avoid lock if clearly a no-op.
        if stage > 0 {
            let mut v = self.value.lock();
            while *v < stage {
                self.cond_var.wait(&mut v);
            }
        }
    }
}

fn sum(db: &impl ParDatabase, key: &'static str) -> usize {
    let mut sum = 0;

    db.signal(db.knobs().sum_signal_on_entry.get());

    db.await(db.knobs().sum_await_on_entry.get());

    for ch in key.chars() {
        sum += db.input(ch);
    }

    if db.knobs().sum_await_cancellation.get() {
        log::debug!("awaiting cancellation");
        while !db.salsa_runtime().is_current_revision_canceled() {
            std::thread::yield_now();
        }
        log::debug!("cancellation observed");
        return std::usize::MAX; // when we are cancelled, we return usize::MAX.
    }

    db.await(db.knobs().sum_await_on_exit.get());

    db.signal(db.knobs().sum_signal_on_exit.get());

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
    fn knobs(&self) -> &KnobsStruct {
        &self.knobs
    }

    fn signal(&self, stage: usize) {
        self.knobs.signal.signal(stage);
    }

    fn await(&self, stage: usize) {
        self.knobs.signal.await(stage);
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
