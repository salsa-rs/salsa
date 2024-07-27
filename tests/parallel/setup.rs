use crossbeam::atomic::AtomicCell;
use salsa::{Database, DatabaseImpl, UserData};

use crate::signal::Signal;

/// Various "knobs" and utilities used by tests to force
/// a certain behavior.
#[salsa::db]
pub(crate) trait KnobsDatabase: Database {
    fn knobs(&self) -> &Knobs;

    fn signal(&self, stage: usize);

    fn wait_for(&self, stage: usize);
}

/// Various "knobs" that can be used to customize how the queries
/// behave on one specific thread. Note that this state is
/// intentionally thread-local (apart from `signal`).
#[derive(Default)]
pub(crate) struct Knobs {
    /// A kind of flexible barrier used to coordinate execution across
    /// threads to ensure we reach various weird states.
    pub(crate) signal: Signal,

    /// When this database is about to block, send this signal.
    pub(crate) signal_on_will_block: AtomicCell<usize>,

    /// When this database has set the cancellation flag, send this signal.
    pub(crate) signal_on_did_cancel: AtomicCell<usize>,
}

impl UserData for Knobs {
    fn salsa_event(db: &DatabaseImpl<Self>, event: &dyn Fn() -> salsa::Event) {
        let event = event();
        match event.kind {
            salsa::EventKind::WillBlockOn { .. } => {
                db.signal(db.signal_on_will_block.load());
            }
            salsa::EventKind::DidSetCancellationFlag => {
                db.signal(db.signal_on_did_cancel.load());
            }
            _ => {}
        }
    }
}

#[salsa::db]
impl KnobsDatabase for DatabaseImpl<Knobs> {
    fn knobs(&self) -> &Knobs {
        self
    }

    fn signal(&self, stage: usize) {
        self.signal.signal(stage);
    }

    fn wait_for(&self, stage: usize) {
        self.signal.wait_for(stage);
    }
}
