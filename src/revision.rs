use std::num::NonZeroUsize;

use crate::sync::atomic::{AtomicUsize, Ordering};

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
    #[inline]
    pub(crate) fn max() -> Self {
        Self::from(usize::MAX)
    }

    #[inline]
    pub(crate) const fn start() -> Self {
        Self {
            // SAFETY: `START` is non-zero.
            generation: unsafe { NonZeroUsize::new_unchecked(START) },
        }
    }

    #[inline]
    pub(crate) fn from(g: usize) -> Self {
        Self {
            generation: NonZeroUsize::new(g).unwrap(),
        }
    }

    #[inline]
    pub(crate) fn from_opt(g: usize) -> Option<Self> {
        NonZeroUsize::new(g).map(|generation| Self { generation })
    }

    #[inline]
    pub(crate) fn next(self) -> Revision {
        Self::from(self.generation.get() + 1)
    }

    #[inline]
    pub(crate) fn as_usize(self) -> usize {
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
        Revision {
            // SAFETY: We know that the value is non-zero because we only ever store `START` which 1, or a
            // Revision which is guaranteed to be non-zero.
            generation: unsafe { NonZeroUsize::new_unchecked(self.data.load(Ordering::Acquire)) },
        }
    }

    pub(crate) fn store(&self, r: Revision) {
        self.data.store(r.as_usize(), Ordering::Release);
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

pub struct MaybeAtomicRevision {
    data: AtomicUsize,
}

impl From<Revision> for MaybeAtomicRevision {
    fn from(value: Revision) -> Self {
        Self {
            data: AtomicUsize::new(value.as_usize()),
        }
    }
}
impl MaybeAtomicRevision {
    pub fn load(&self) -> Revision {
        Revision {
            // SAFETY: we only store Revision.as_usize, so it always valid
            generation: unsafe { NonZeroUsize::new_unchecked(self.data.load(Ordering::Relaxed)) }
        }
    }

    pub fn store(&self, revision: Revision) {
        self.data.store(revision.as_usize(), Ordering::Relaxed);
    }

    /// # Safety
    /// Caller must ensure that there are no unsyncronized writes to this variable.
    pub unsafe fn non_atomic_load(&self) -> Revision {
        Revision {
            // SAFETY: we only store Revision.as_usize, so it always valid
            generation: unsafe { NonZeroUsize::new_unchecked(std::ptr::read(self.data.as_ptr())) }
        }
    }

    pub fn non_atomic_store(&mut self, revision: Revision) {
        // SAFETY: we have &mut self, so there can't be any other refs
        unsafe {
            std::ptr::write(self.data.as_ptr(), revision.as_usize());
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_atomic_revision() {
        let val = OptionalAtomicRevision::new(Some(Revision::start()));
        assert_eq!(val.load(), Some(Revision::start()));
        assert_eq!(val.swap(None), Some(Revision::start()));
        assert_eq!(val.load(), None);
        assert_eq!(val.swap(Some(Revision::start())), None);
        assert_eq!(val.load(), Some(Revision::start()));
        assert_eq!(
            val.compare_exchange(Some(Revision::start()), None),
            Ok(Some(Revision::start()))
        );
        assert_eq!(
            val.compare_exchange(Some(Revision::start()), None),
            Err(None)
        );
    }
}
