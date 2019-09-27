use crate::signal::Signal;
use salsa::Database;
use salsa::ParallelDatabase;
use salsa::Snapshot;
use std::cell::Cell;
use std::sync::Arc;

#[salsa::query_group(Par)]
pub(crate) trait ParDatabase: Knobs + salsa::ParallelDatabase {
    #[salsa::input]
    fn input(&self, key: char) -> usize;

    fn sum(&self, key: &'static str) -> usize;

    /// Invokes `sum`
    fn sum2(&self, key: &'static str) -> usize;

    /// Invokes `sum` but doesn't really care about the result.
    fn sum2_drop_sum(&self, key: &'static str) -> usize;

    /// Invokes `sum2`
    fn sum3(&self, key: &'static str) -> usize;

    /// Invokes `sum2_drop_sum`
    fn sum3_drop_sum(&self, key: &'static str) -> usize;

    fn snapshot_me(&self) -> ();
}

#[derive(PartialEq, Eq)]
pub(crate) struct Canceled;

impl Canceled {
    fn throw() -> ! {
        // Don't print backtrace
        std::panic::resume_unwind(Box::new(Canceled));
    }
}

/// Various "knobs" and utilities used by tests to force
/// a certain behavior.
pub(crate) trait Knobs {
    fn knobs(&self) -> &KnobsStruct;

    fn signal(&self, stage: usize);

    fn wait_for(&self, stage: usize);
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelationFlag {
    Down,
    Panic,
    SpecialValue,
}

impl Default for CancelationFlag {
    fn default() -> CancelationFlag {
        CancelationFlag::Down
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

    /// When this database is about to block, send a signal.
    pub(crate) signal_on_will_block: Cell<usize>,

    /// Invocations of `sum` will signal this stage on entry.
    pub(crate) sum_signal_on_entry: Cell<usize>,

    /// Invocations of `sum` will wait for this stage on entry.
    pub(crate) sum_wait_for_on_entry: Cell<usize>,

    /// If true, invocations of `sum` will panic before they exit.
    pub(crate) sum_should_panic: Cell<bool>,

    /// If true, invocations of `sum` will wait for cancellation before
    /// they exit.
    pub(crate) sum_wait_for_cancellation: Cell<CancelationFlag>,

    /// Invocations of `sum` will wait for this stage prior to exiting.
    pub(crate) sum_wait_for_on_exit: Cell<usize>,

    /// Invocations of `sum` will signal this stage prior to exiting.
    pub(crate) sum_signal_on_exit: Cell<usize>,

    /// Invocations of `sum3_drop_sum` will panic unconditionally
    pub(crate) sum3_drop_sum_should_panic: Cell<bool>,
}

fn sum(db: &impl ParDatabase, key: &'static str) -> usize {
    let mut sum = 0;

    db.signal(db.knobs().sum_signal_on_entry.get());

    db.wait_for(db.knobs().sum_wait_for_on_entry.get());

    if db.knobs().sum_should_panic.get() {
        panic!("query set to panic before exit")
    }

    for ch in key.chars() {
        sum += db.input(ch);
    }

    match db.knobs().sum_wait_for_cancellation.get() {
        CancelationFlag::Down => (),
        flag => {
            log::debug!("waiting for cancellation");
            while !db.salsa_runtime().is_current_revision_canceled() {
                std::thread::yield_now();
            }
            log::debug!("observed cancelation");
            if flag == CancelationFlag::Panic {
                Canceled::throw();
            }
        }
    }

    // Check for cancelation and return MAX if so. Note that we check
    // for cancelation *deterministically* -- but if
    // `sum_wait_for_cancellation` is set, we will block
    // beforehand. Deterministic execution is a requirement for valid
    // salsa user code. It's also important to some tests that `sum`
    // *attempts* to invoke `is_current_revision_canceled` even if we
    // know it will not be canceled, because that helps us keep the
    // accounting up to date.
    if db.salsa_runtime().is_current_revision_canceled() {
        return std::usize::MAX; // when we are cancelled, we return usize::MAX.
    }

    db.wait_for(db.knobs().sum_wait_for_on_exit.get());

    db.signal(db.knobs().sum_signal_on_exit.get());

    sum
}

fn sum2(db: &impl ParDatabase, key: &'static str) -> usize {
    db.sum(key)
}

fn sum2_drop_sum(db: &impl ParDatabase, key: &'static str) -> usize {
    let _ = db.sum(key);
    22
}

fn sum3(db: &impl ParDatabase, key: &'static str) -> usize {
    db.sum2(key)
}

fn sum3_drop_sum(db: &impl ParDatabase, key: &'static str) -> usize {
    if db.knobs().sum3_drop_sum_should_panic.get() {
        panic!("sum3_drop_sum executed")
    }
    db.sum2_drop_sum(key)
}

fn snapshot_me(db: &impl ParDatabase) {
    // this should panic
    db.snapshot();
}

#[salsa::database(Par)]
#[derive(Default)]
pub(crate) struct ParDatabaseImpl {
    runtime: salsa::Runtime<ParDatabaseImpl>,
    knobs: KnobsStruct,
}

impl Database for ParDatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<Self> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<Self> {
        &mut self.runtime
    }

    fn salsa_event(&self, event_fn: impl Fn() -> salsa::Event<Self>) {
        let event = event_fn();
        match event.kind {
            salsa::EventKind::WillBlockOn { .. } => {
                self.signal(self.knobs().signal_on_will_block.get());
            }

            _ => {}
        }
    }

    fn on_propagated_panic(&self) -> ! {
        Canceled::throw()
    }
}

impl ParallelDatabase for ParDatabaseImpl {
    fn snapshot(&self) -> Snapshot<Self> {
        Snapshot::new(ParDatabaseImpl {
            runtime: self.runtime.snapshot(self),
            knobs: self.knobs.clone(),
        })
    }
}

impl Knobs for ParDatabaseImpl {
    fn knobs(&self) -> &KnobsStruct {
        &self.knobs
    }

    fn signal(&self, stage: usize) {
        self.knobs.signal.signal(stage);
    }

    fn wait_for(&self, stage: usize) {
        self.knobs.signal.wait_for(stage);
    }
}
