use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Value of the initial revision, as a u64. We don't use 0
/// because we want to use a `NonZeroUsize`.
const START: usize = 1;

/// A unique identifier for the current version of the database.
///
/// Each time an input is changed, the revision number is incremented.
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

    pub(crate) fn from_opt(g: usize) -> Option<Self> {
        NonZeroUsize::new(g).map(|generation| Self { generation })
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

impl From<Revision> for AtomicRevision {
    fn from(value: Revision) -> Self {
        Self {
            data: AtomicUsize::new(value.as_usize()),
        }
    }
}

impl AtomicRevision {
    pub(crate) const fn start() -> Self {
        Self {
            data: AtomicUsize::new(START),
        }
    }

    pub(crate) fn load(&self) -> Revision {
        // Safety: We know that the value is non-zero because we only ever store `START` which 1, or a
        // Revision which is guaranteed to be non-zero.
        Revision {
            generation: unsafe { NonZeroUsize::new_unchecked(self.data.load(Ordering::Acquire)) },
        }
    }

    pub(crate) fn store(&self, r: Revision) {
        self.data.store(r.as_usize(), Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub(crate) struct OptionalAtomicRevision {
    data: AtomicUsize,
}

impl From<Revision> for OptionalAtomicRevision {
    fn from(value: Revision) -> Self {
        Self {
            data: AtomicUsize::new(value.as_usize()),
        }
    }
}

impl OptionalAtomicRevision {
    pub(crate) fn new(revision: Option<Revision>) -> Self {
        Self {
            data: AtomicUsize::new(revision.map_or(0, |r| r.as_usize())),
        }
    }

    pub(crate) fn load(&self) -> Option<Revision> {
        Revision::from_opt(self.data.load(Ordering::Acquire))
    }

    pub(crate) fn swap(&self, val: Option<Revision>) -> Option<Revision> {
        Revision::from_opt(
            self.data
                .swap(val.map_or(0, |r| r.as_usize()), Ordering::AcqRel),
        )
    }

    pub(crate) fn compare_exchange(
        &self,
        current: Option<Revision>,
        new: Option<Revision>,
    ) -> Result<Option<Revision>, Option<Revision>> {
        self.data
            .compare_exchange(
                current.map_or(0, |r| r.as_usize()),
                new.map_or(0, |r| r.as_usize()),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(Revision::from_opt)
            .map_err(Revision::from_opt)
    }
}
