use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use salsa::Database;

use crate::signal::Signal;

/// Various "knobs" and utilities used by tests to force
/// a certain behavior.
#[salsa::db]
pub(crate) trait KnobsDatabase: Database {
    /// Signal that we are entering stage `stage`.
    fn signal(&self, stage: usize);

    /// Wait until we reach stage `stage` (no-op if we have already reached that stage).
    fn wait_for(&self, stage: usize);
}

/// A database containing various "knobs" that can be used to customize how the queries
/// behave on one specific thread. Note that this state is
/// intentionally thread-local (apart from `signal`).
#[salsa::db]
#[derive(Default)]
pub(crate) struct Knobs {
    storage: salsa::Storage<Self>,

    /// A kind of flexible barrier used to coordinate execution across
    /// threads to ensure we reach various weird states.
    pub(crate) signal: Arc<Signal>,

    /// When this database is about to block, send this signal.
    signal_on_will_block: AtomicUsize,

    /// When this database has set the cancellation flag, send this signal.
    signal_on_did_cancel: AtomicUsize,
}

impl Knobs {
    pub fn signal_on_did_cancel(&self, stage: usize) {
        self.signal_on_did_cancel.store(stage, Ordering::Release);
    }

    pub fn signal_on_will_block(&self, stage: usize) {
        self.signal_on_will_block.store(stage, Ordering::Release);
    }
}

impl Clone for Knobs {
    #[track_caller]
    fn clone(&self) -> Self {
        // To avoid mistakes, check that when we clone, we haven't customized this behavior yet
        assert_eq!(self.signal_on_will_block.load(Ordering::Acquire), 0);
        assert_eq!(self.signal_on_did_cancel.load(Ordering::Acquire), 0);
        Self {
            storage: self.storage.clone(),
            signal: self.signal.clone(),
            signal_on_will_block: AtomicUsize::new(0),
            signal_on_did_cancel: AtomicUsize::new(0),
        }
    }
}

#[salsa::db]
impl salsa::Database for Knobs {
    fn salsa_event(&self, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        match event.kind {
            salsa::EventKind::WillBlockOn { .. } => {
                self.signal(self.signal_on_will_block.load(Ordering::Acquire));
            }
            salsa::EventKind::DidSetCancellationFlag => {
                self.signal(self.signal_on_did_cancel.load(Ordering::Acquire));
            }
            _ => {}
        }
    }
}

#[salsa::db]
impl KnobsDatabase for Knobs {
    fn signal(&self, stage: usize) {
        self.signal.signal(stage);
    }

    fn wait_for(&self, stage: usize) {
        self.signal.wait_for(stage);
    }
}
