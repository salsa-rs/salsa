//! Random-replacement eviction for volatile query values.

use arc_swap::ArcSwapOption;

use crate::Id;
use crate::hash::hash;
use crate::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::EvictionPolicy;

/// Keeps up to `capacity` randomly selected query values resident.
///
/// Cache hits require no bookkeeping. Once every slot has been filled, each
/// insertion replaces one pseudo-randomly selected resident value. A value
/// that cannot be evicted, such as one with escaped accumulated values,
/// remains alive until the next revision but no longer occupies a slot.
pub struct Volatile {
    slots: Box<[AtomicU64]>,
    next: AtomicUsize,
}

impl Volatile {
    fn slot(&self, insertion: usize) -> usize {
        if insertion < self.slots.len() {
            insertion
        } else {
            hash(&insertion) as usize % self.slots.len()
        }
    }
}

impl EvictionPolicy for Volatile {
    type Value<T: Send + Sync> = ArcSwapOption<T>;

    const STORES_VALUE_INLINE: bool = false;
    const RETIRES_VALUES: bool = true;

    fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity).map(|_| AtomicU64::new(0)).collect(),
            next: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn record_use(&self, _id: Id) {}

    #[inline]
    fn record_insert(&self, id: Id) -> Option<Id> {
        if self.slots.is_empty() {
            return None;
        }

        let insertion = self.next.fetch_add(1, Ordering::Relaxed);
        let old = self.slots[self.slot(insertion)].swap(id.as_bits(), Ordering::Relaxed);

        (old != 0 && old != id.as_bits()).then(|| Id::from_bits(old))
    }

    fn set_capacity(&mut self, _capacity: usize) {}

    fn for_each_evicted(&mut self, _cb: impl FnMut(Id)) {}
}

#[cfg(test)]
mod tests {
    use super::{EvictionPolicy, Volatile};
    use crate::Id;

    fn id(index: u32) -> Id {
        // SAFETY: The test IDs are small and distinct.
        unsafe { Id::from_index(index) }
    }

    #[test]
    fn fills_all_slots_before_evicting() {
        let policy = Volatile::new(4);

        for index in 0..4 {
            assert_eq!(policy.record_insert(id(index)), None);
        }

        let evicted = policy.record_insert(id(4)).unwrap();
        assert!(evicted.index() < 4);
    }

    #[test]
    fn replaces_one_value_per_insert_after_filling() {
        let policy = Volatile::new(4);

        for index in 0..4 {
            assert_eq!(policy.record_insert(id(index)), None);
        }

        for index in 4..20 {
            assert!(policy.record_insert(id(index)).is_some());
        }
    }

    #[test]
    fn zero_capacity_disables_eviction() {
        let policy = Volatile::new(0);
        assert_eq!(policy.record_insert(id(0)), None);
    }
}
