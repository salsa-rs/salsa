use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

/// Value if the initial revision, as a u64. We don't use 0
/// because we want to use a `NonZeroU64`.
const START_U64: u64 = 1;

/// A unique identifier for the current version of the database; each
/// time an input is changed, the revision number is incremented.
/// `Revision` is used internally to track which values may need to be
/// recomputed, but not something you should have to interact with
/// directly as a user of salsa.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision {
    generation: NonZeroU64,
}

impl Revision {
    pub(crate) fn start() -> Self {
        Self::from(START_U64)
    }

    pub(crate) fn from(g: u64) -> Self {
        Self {
            generation: NonZeroU64::new(g).unwrap(),
        }
    }

    pub(crate) fn next(self) -> Revision {
        Self::from(self.generation.get() + 1)
    }

    fn as_u64(self) -> u64 {
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
    data: AtomicU64,
}

impl AtomicRevision {
    pub(crate) fn start() -> Self {
        Self {
            data: AtomicU64::new(START_U64),
        }
    }

    pub(crate) fn load(&self) -> Revision {
        Revision::from(self.data.load(Ordering::SeqCst))
    }

    pub(crate) fn store(&self, r: Revision) {
        self.data.store(r.as_u64(), Ordering::SeqCst);
    }

    /// Increment by 1, returning previous value.
    pub(crate) fn fetch_then_increment(&self) -> Revision {
        let v = self.data.fetch_add(1, Ordering::SeqCst);
        assert!(v != u64::max_value(), "revision overflow");
        Revision::from(v)
    }
}
