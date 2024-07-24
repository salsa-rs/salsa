use crossbeam::atomic::AtomicCell;

use crate::signal::Signal;

/// Various "knobs" and utilities used by tests to force
/// a certain behavior.
pub(crate) trait Knobs {
    fn knobs(&self) -> &KnobsStruct;

    fn signal(&self, stage: usize);

    fn wait_for(&self, stage: usize);
}

/// Various "knobs" that can be used to customize how the queries
/// behave on one specific thread. Note that this state is
/// intentionally thread-local (apart from `signal`).
#[derive(Default)]
pub(crate) struct KnobsStruct {
    /// A kind of flexible barrier used to coordinate execution across
    /// threads to ensure we reach various weird states.
    pub(crate) signal: Signal,

    /// When this database is about to block, send a signal.
    pub(crate) signal_on_will_block: AtomicCell<usize>,
}

#[salsa::db]
#[derive(Default)]
pub(crate) struct Database {
    storage: salsa::Storage<Self>,
    knobs: KnobsStruct,
}

#[salsa::db]
impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        if let salsa::EventKind::WillBlockOn { .. } = event.kind {
            self.signal(self.knobs().signal_on_will_block.load());
        }
    }
}

impl Knobs for Database {
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
