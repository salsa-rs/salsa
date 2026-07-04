//! Immediate SIEVE eviction for volatile query values.

use arc_swap::ArcSwapOption;

use crate::Id;

use super::{EvictionPolicy, Sieve};

/// A SIEVE policy that returns victims for immediate value reclamation.
///
/// Volatile callers own their result handles, so they can clear a selected
/// victim within the current revision. The resident order, moving hand, and
/// visited-bit behavior are shared with [`Sieve`].
pub struct Volatile {
    sieve: Sieve,
}

impl EvictionPolicy for Volatile {
    type Value<T: Send + Sync> = ArcSwapOption<T>;

    const STORES_VALUE_INLINE: bool = false;
    const RETIRES_VALUES: bool = true;

    fn new(capacity: usize) -> Self {
        Self {
            sieve: Sieve::new(capacity),
        }
    }

    #[inline]
    fn record_volatile_use(&self, id: Id) -> Option<Id> {
        self.sieve.record_immediate_use(id)
    }

    fn set_tuning(&mut self, _capacity: usize) {}

    fn for_each_evicted(&mut self, _evict: impl FnMut(Id)) {}
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::*;

    fn id(index: u32) -> Id {
        // SAFETY: Test indices are within `Id`'s valid range.
        unsafe { Id::from_index(index) }
    }

    #[test]
    fn returns_sieve_victims_immediately() {
        let volatile = Volatile::new(3);
        let oldest = id(0);
        let middle = id(1);
        let newest = id(2);

        assert_eq!(volatile.record_volatile_use(oldest), None);
        assert_eq!(volatile.record_volatile_use(middle), None);
        assert_eq!(volatile.record_volatile_use(newest), None);

        assert_eq!(volatile.record_volatile_use(oldest), None);
        assert_eq!(volatile.record_volatile_use(id(3)), Some(middle));
    }

    #[test]
    fn does_not_defer_victims_to_revision_boundaries() {
        let mut volatile = Volatile::new(1);

        assert_eq!(volatile.record_volatile_use(id(0)), None);
        assert_eq!(volatile.record_volatile_use(id(1)), Some(id(0)));

        volatile.for_each_evicted(|_| panic!("volatile victim should already be returned"));
    }
}
