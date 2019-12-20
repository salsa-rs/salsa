use crate::signal::Signal;
use crossbeam::atomic::AtomicCell;
use salsa::Database;
use salsa::ParallelDatabase;
use salsa::Snapshot;
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
    fn knobs(&self) -> Arc<KnobsStruct>;

    fn signal(&self, stage: usize);

    fn wait_for(&self, stage: usize);
}

pub(crate) trait WithValue<T> {
    fn with_value<R>(&self, value: T, closure: impl FnOnce() -> R) -> R;
}

impl<T> WithValue<T> for AtomicCell<T>
where
    T: Copy,
{
    fn with_value<R>(&self, value: T, closure: impl FnOnce() -> R) -> R {
        let old_value = self.swap(value);

        let result = closure();

        self.store(old_value);

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
#[derive(Default)]
pub(crate) struct KnobsStruct {
    /// A kind of flexible barrier used to coordinate execution across
    /// threads to ensure we reach various weird states.
    pub(crate) signal: Arc<Signal>,

    /// When this database is about to block, send a signal.
    pub(crate) signal_on_will_block: AtomicCell<usize>,

    /// Invocations of `sum` will signal this stage on entry.
    pub(crate) sum_signal_on_entry: AtomicCell<usize>,

    /// Invocations of `sum` will wait for this stage on entry.
    pub(crate) sum_wait_for_on_entry: AtomicCell<usize>,

    /// If true, invocations of `sum` will panic before they exit.
    pub(crate) sum_should_panic: AtomicCell<bool>,

    /// If true, invocations of `sum` will wait for cancellation before
    /// they exit.
    pub(crate) sum_wait_for_cancellation: AtomicCell<CancelationFlag>,

    /// Invocations of `sum` will wait for this stage prior to exiting.
    pub(crate) sum_wait_for_on_exit: AtomicCell<usize>,

    /// Invocations of `sum` will signal this stage prior to exiting.
    pub(crate) sum_signal_on_exit: AtomicCell<usize>,

    /// Invocations of `sum3_drop_sum` will panic unconditionally
    pub(crate) sum3_drop_sum_should_panic: AtomicCell<bool>,
}

impl Clone for KnobsStruct {
    fn clone(&self) -> Self {
        KnobsStruct {
            signal: self.signal.clone(),
            signal_on_will_block: AtomicCell::new(self.signal_on_will_block.load()),
            sum_signal_on_entry: AtomicCell::new(self.sum_signal_on_entry.load()),
            sum_wait_for_on_entry: AtomicCell::new(self.sum_wait_for_on_entry.load()),
            sum_should_panic: AtomicCell::new(self.sum_should_panic.load()),
            sum_wait_for_cancellation: AtomicCell::new(self.sum_wait_for_cancellation.load()),
            sum_wait_for_on_exit: AtomicCell::new(self.sum_wait_for_on_exit.load()),
            sum_signal_on_exit: AtomicCell::new(self.sum_signal_on_exit.load()),
            sum3_drop_sum_should_panic: AtomicCell::new(self.sum3_drop_sum_should_panic.load()),
        }
    }
}

fn sum(db: &mut impl ParDatabase, key: &'static str) -> usize {
    let mut sum = 0;

    db.signal(db.knobs().sum_signal_on_entry.load());

    db.wait_for(db.knobs().sum_wait_for_on_entry.load());

    if db.knobs().sum_should_panic.load() {
        panic!("query set to panic before exit")
    }

    for ch in key.chars() {
        sum += db.input(ch);
    }

    match db.knobs().sum_wait_for_cancellation.load() {
        CancelationFlag::Down => (),
        flag => {
            log::debug!("waiting for cancellation");
            while !db.salsa_runtime_mut().is_current_revision_canceled() {
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
    if db.salsa_runtime_mut().is_current_revision_canceled() {
        return std::usize::MAX; // when we are cancelled, we return usize::MAX.
    }

    db.wait_for(db.knobs().sum_wait_for_on_exit.load());

    db.signal(db.knobs().sum_signal_on_exit.load());

    sum
}

fn sum2(db: &mut impl ParDatabase, key: &'static str) -> usize {
    db.sum(key)
}

fn sum2_drop_sum(db: &mut impl ParDatabase, key: &'static str) -> usize {
    let _ = db.sum(key);
    22
}

fn sum3(db: &mut impl ParDatabase, key: &'static str) -> usize {
    db.sum2(key)
}

fn sum3_drop_sum(db: &mut impl ParDatabase, key: &'static str) -> usize {
    if db.knobs().sum3_drop_sum_should_panic.load() {
        panic!("sum3_drop_sum executed")
    }
    db.sum2_drop_sum(key)
}

fn snapshot_me(db: &mut impl ParDatabase) {
    // this should panic
    db.snapshot();
}

#[salsa::database(Par)]
#[derive(Default)]
pub(crate) struct ParDatabaseImpl {
    runtime: salsa::Runtime<ParDatabaseImpl>,
    knobs: Arc<KnobsStruct>,
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
                self.signal(self.knobs().signal_on_will_block.load());
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
            knobs: Arc::new(KnobsStruct::clone(&self.knobs)),
        })
    }
    fn fork(&self, forker: salsa::ForkState<Self>) -> salsa::Snapshot<Self> {
        salsa::Snapshot::new(Self {
            runtime: self.runtime.fork(self, forker),
            knobs: Arc::new(KnobsStruct::clone(&self.knobs)),
        })
    }
}

impl Knobs for ParDatabaseImpl {
    fn knobs(&self) -> Arc<KnobsStruct> {
        self.knobs.clone()
    }

    fn signal(&self, stage: usize) {
        self.knobs.signal.signal(stage);
    }

    fn wait_for(&self, stage: usize) {
        self.knobs.signal.wait_for(stage);
    }
}
