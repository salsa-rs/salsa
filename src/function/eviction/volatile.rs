//! Second-chance eviction for volatile query values.

use arc_swap::ArcSwapOption;

use crate::Id;
use crate::hash::hash;
use crate::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::EvictionPolicy;

const ASSOCIATIVITY: usize = 4;

/// Keeps recently accessed query values resident.
///
/// IDs compete within small hash buckets. Accesses set a reference bit. When
/// admitting a new ID to a full bucket, referenced residents get one second
/// chance before an unreferenced resident is replaced.
pub struct Volatile {
    slots: Box<[AtomicU64]>,
    referenced: Box<[AtomicBool]>,
    bucket_count: usize,
}

impl Volatile {
    fn bucket(&self, id: Id) -> std::ops::Range<usize> {
        let bucket = hash(&id) as usize % self.bucket_count;
        let start = bucket * self.slots.len() / self.bucket_count;
        let end = (bucket + 1) * self.slots.len() / self.bucket_count;
        start..end
    }

    fn record_access(&self, id: Id) -> Option<Id> {
        let bits = id.as_bits();

        'retry: loop {
            let bucket = self.bucket(id);

            for index in bucket.clone() {
                let resident = self.slots[index].load(Ordering::Relaxed);
                if resident == bits {
                    self.referenced[index].store(true, Ordering::Relaxed);
                    return None;
                }

                if resident == 0 {
                    self.referenced[index].store(true, Ordering::Relaxed);
                    match self.slots[index].compare_exchange(
                        0,
                        bits,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => return None,
                        Err(_) => continue 'retry,
                    }
                }
            }

            let start = hash(&bits) as usize % bucket.len();
            for offset in 0..bucket.len() {
                let index = bucket.start + (start + offset) % bucket.len();
                if self.referenced[index].swap(false, Ordering::Relaxed) {
                    continue;
                }

                let resident = self.slots[index].load(Ordering::Relaxed);
                if resident == bits {
                    self.referenced[index].store(true, Ordering::Relaxed);
                    return None;
                }
                if resident == 0 {
                    continue 'retry;
                }

                self.referenced[index].store(true, Ordering::Relaxed);
                match self.slots[index].compare_exchange(
                    resident,
                    bits,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Some(Id::from_bits(resident)),
                    Err(_) => continue 'retry,
                }
            }

            // Every resident received a second chance. Replace the first one
            // on the next pass rather than allowing admission to spin while
            // concurrent readers keep setting reference bits.
            let index = bucket.start + start;
            let resident = self.slots[index].load(Ordering::Relaxed);
            if resident == bits {
                self.referenced[index].store(true, Ordering::Relaxed);
                return None;
            }
            if resident == 0 {
                continue 'retry;
            }

            self.referenced[index].store(true, Ordering::Relaxed);
            match self.slots[index].compare_exchange(
                resident,
                bits,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Id::from_bits(resident)),
                Err(_) => continue 'retry,
            }
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
            referenced: (0..capacity).map(|_| AtomicBool::new(false)).collect(),
            bucket_count: (capacity / ASSOCIATIVITY).max(1),
        }
    }

    #[inline(always)]
    fn record_use(&self, id: Id) {
        if !self.slots.is_empty() {
            self.record_access(id);
        }
    }

    #[inline]
    fn record_volatile_use(&self, id: Id) -> Option<Id> {
        if self.slots.is_empty() {
            return None;
        }

        self.record_access(id)
    }

    #[inline(always)]
    fn record_insert(&self, _id: Id) -> Option<Id> {
        None
    }

    fn set_capacity(&mut self, _capacity: usize) {}

    fn for_each_evicted(&mut self, _cb: impl FnMut(Id)) {}
}

#[cfg(all(test, not(feature = "shuttle")))]
mod tests {
    use super::{EvictionPolicy, Ordering, Volatile};
    use crate::Id;

    fn id(index: u32) -> Id {
        // SAFETY: The test IDs are small and distinct.
        unsafe { Id::from_index(index) }
    }

    fn slot(policy: &Volatile, id: Id) -> usize {
        policy
            .bucket(id)
            .find(|&index| policy.slots[index].load(Ordering::Relaxed) == id.as_bits())
            .expect("ID should be resident")
    }

    #[test]
    fn fills_all_slots_before_considering_eviction() {
        let policy = Volatile::new(4);

        for index in 0..4 {
            assert_eq!(policy.record_volatile_use(id(index)), None);
        }

        assert!(policy.record_volatile_use(id(4)).is_some());
    }

    #[test]
    fn referenced_resident_gets_a_second_chance() {
        let policy = Volatile::new(2);
        let referenced = id(0);
        let unreferenced = id(1);

        policy.record_volatile_use(referenced);
        policy.record_volatile_use(unreferenced);
        policy.referenced[slot(&policy, unreferenced)].store(false, Ordering::Relaxed);

        assert_eq!(policy.record_volatile_use(id(2)), Some(unreferenced));
        assert!(policy.record_volatile_use(referenced).is_none());
    }

    #[test]
    fn sweep_clears_reference_bits() {
        let policy = Volatile::new(2);
        let first = id(0);
        let second = id(1);

        policy.record_volatile_use(first);
        policy.record_volatile_use(second);
        policy.record_volatile_use(id(2));

        assert!(
            [first, second]
                .into_iter()
                .filter_map(|id| {
                    policy
                        .bucket(id)
                        .find(|&index| policy.slots[index].load(Ordering::Relaxed) == id.as_bits())
                })
                .all(|index| !policy.referenced[index].load(Ordering::Relaxed))
        );
    }

    #[test]
    fn zero_capacity_disables_eviction() {
        let policy = Volatile::new(0);
        assert_eq!(policy.record_volatile_use(id(0)), None);
    }
}
