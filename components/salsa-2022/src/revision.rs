use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Value of the initial revision, as a u64. We don't use 0
/// because we want to use a `NonZeroUsize`.
const START: usize = 1;

/// A unique identifier for the current version of the database; each
/// time an input is changed, the revision number is incremented.
/// `Revision` is used internally to track which values may need to be
/// recomputed, but is not something you should have to interact with
/// directly as a user of salsa.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision {
    generation: NonZeroUsize,
}

impl Revision {
    pub(crate) fn start() -> Self {
        Self::from(START)
    }

    pub(crate) fn from(g: usize) -> Self {
        Self {
            generation: NonZeroUsize::new(g).unwrap(),
        }
    }

    pub(crate) fn next(self) -> Revision {
        Self::from(self.generation.get() + 1)
    }

    fn as_usize(self) -> usize {
        self.generation.get()
    }
}

impl std::fmt::Debug for Revision {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "R{}", self.generation)
    }
}

#[derive(Debug)]
pub(crate) struct AtomicRevision {
    data: AtomicUsize,
}

impl AtomicRevision {
    pub(crate) fn start() -> Self {
        Self {
            data: AtomicUsize::new(START),
        }
    }

    pub(crate) fn load(&self) -> Revision {
        Revision::from(self.data.load(Ordering::SeqCst))
    }

    pub(crate) fn store(&self, r: Revision) {
        self.data.store(r.as_usize(), Ordering::SeqCst);
    }
}
