use std::sync::{atomic::AtomicUsize, Arc};

use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

use crate::{durability::Durability, key::DependencyIndex, revision::AtomicRevision};

use super::{dependency_graph::DependencyGraph, local_state::EdgeKind};

/// State that will be common to all threads (when we support multiple threads)
#[derive(Debug)]
pub(super) struct SharedState {
    /// Stores the next id to use for a snapshotted runtime (starts at 1).
    pub(super) next_id: AtomicUsize,

    /// Vector we can clone
    pub(super) empty_dependencies: Arc<[(EdgeKind, DependencyIndex)]>,

    /// Set to true when the current revision has been canceled.
    /// This is done when we an input is being changed. The flag
    /// is set back to false once the input has been changed.
    pub(super) revision_canceled: AtomicCell<bool>,

    /// Stores the "last change" revision for values of each duration.
    /// This vector is always of length at least 1 (for Durability 0)
    /// but its total length depends on the number of durations. The
    /// element at index 0 is special as it represents the "current
    /// revision".  In general, we have the invariant that revisions
    /// in here are *declining* -- that is, `revisions[i] >=
    /// revisions[i + 1]`, for all `i`. This is because when you
    /// modify a value with durability D, that implies that values
    /// with durability less than D may have changed too.
    pub(super) revisions: Vec<AtomicRevision>,

    /// The dependency graph tracks which runtimes are blocked on one
    /// another, waiting for queries to terminate.
    pub(super) dependency_graph: Mutex<DependencyGraph>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self::with_durabilities(Durability::LEN)
    }
}

impl SharedState {
    fn with_durabilities(durabilities: usize) -> Self {
        SharedState {
            next_id: AtomicUsize::new(1),
            empty_dependencies: None.into_iter().collect(),
            revision_canceled: Default::default(),
            revisions: (0..durabilities).map(|_| AtomicRevision::start()).collect(),
            dependency_graph: Default::default(),
        }
    }
}
