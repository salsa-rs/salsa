//! Least Recently Used (LRU) eviction policy.
//!
//! This policy tracks the most recently accessed items and evicts
//! the least recently used ones when the cache exceeds its capacity.

use std::num::NonZeroUsize;

use crate::Id;
use crate::hash::FxLinkedHashSet;
use crate::sync::Mutex;
use crate::sync::atomic::{AtomicU64, Ordering};

use super::{EvictionPolicy, HasCapacity};

const MAX_SHARD_COUNT: usize = 32;

/// Least Recently Used eviction policy.
///
/// When the number of memoized values exceeds the configured capacity,
/// the least recently accessed values are evicted at the start of each
/// new revision.
///
/// Recency is tracked independently across shards. This makes the policy an
/// approximation of a global LRU, but prevents unrelated keys from contending
/// on a single lock during parallel query execution.
pub struct Lru {
    capacity: Option<NonZeroUsize>,
    shards: Box<[LruShard]>,
}

struct LruShard {
    last_used: AtomicU64,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl Default for LruShard {
    fn default() -> Self {
        Self {
            last_used: AtomicU64::new(u64::MAX),
            set: Mutex::default(),
        }
    }
}

impl Lru {
    #[inline(never)]
    fn insert(&self, id: Id) {
        let shard = &self.shards[id.index() as usize % self.shards.len()];
        let bits = id.as_bits();

        if shard.last_used.load(Ordering::Acquire) == bits {
            return;
        }

        let mut set = shard.set.lock();
        if shard.last_used.load(Ordering::Relaxed) != bits {
            set.insert(id);
            shard.last_used.store(bits, Ordering::Release);
        }
    }
}

impl EvictionPolicy for Lru {
    fn new(cap: usize) -> Self {
        Self {
            capacity: NonZeroUsize::new(cap),
            shards: (0..cap.min(MAX_SHARD_COUNT))
                .map(|_| LruShard::default())
                .collect(),
        }
    }

    #[inline(always)]
    fn record_use(&self, id: Id) {
        if self.capacity.is_some() {
            debug_assert!(!self.shards.is_empty());
            self.insert(id);
        }
    }

    fn set_capacity(&mut self, capacity: usize) {
        self.capacity = NonZeroUsize::new(capacity);
        let shard_count = capacity.min(MAX_SHARD_COUNT);
        if self.shards.len() != shard_count {
            let mut shards: Box<[LruShard]> =
                (0..shard_count).map(|_| LruShard::default()).collect();

            if !shards.is_empty() {
                let shard_count = shards.len();
                for old_shard in &mut self.shards {
                    let old_set = old_shard.set.get_mut();
                    while let Some(id) = old_set.pop_front() {
                        let shard = &mut shards[id.index() as usize % shard_count];
                        shard.set.get_mut().insert(id);
                        *shard.last_used.get_mut() = id.as_bits();
                    }
                }
            }

            self.shards = shards;
        }
    }

    fn for_each_evicted(&mut self, mut cb: impl FnMut(Id)) {
        let Some(cap) = self.capacity else {
            return;
        };

        let capacity_per_shard = cap.get() / self.shards.len();
        let shards_with_extra_capacity = cap.get() % self.shards.len();

        for (index, shard) in self.shards.iter_mut().enumerate() {
            let shard_capacity =
                capacity_per_shard + usize::from(index < shards_with_extra_capacity);
            let set = shard.set.get_mut();
            while set.len() > shard_capacity {
                if let Some(id) = set.pop_front() {
                    cb(id);
                }
            }
            if set.is_empty() {
                *shard.last_used.get_mut() = u64::MAX;
            }
        }
    }
}

impl HasCapacity for Lru {}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> Id {
        // SAFETY: Test indexes are well below `Id::MAX_U32`.
        unsafe { Id::from_index(index) }
    }

    fn retained_count(lru: &mut Lru) -> usize {
        lru.shards
            .iter_mut()
            .map(|shard| shard.set.get_mut().len())
            .sum()
    }

    #[test]
    fn capacity_smaller_than_shard_count() {
        let mut lru = Lru::new(1);
        lru.record_use(id(0));
        lru.for_each_evicted(|_| panic!("the only entry should be retained"));
        assert_eq!(retained_count(&mut lru), 1);

        lru.record_use(id(1));
        let mut evicted = Vec::new();
        lru.for_each_evicted(|id| evicted.push(id));
        assert_eq!(evicted, [id(0)]);
        assert_eq!(retained_count(&mut lru), 1);
    }

    #[test]
    fn capacity_is_split_across_shards() {
        let mut lru = Lru::new(35);
        for index in 0..70 {
            lru.record_use(id(index));
        }

        let mut evicted = Vec::new();
        lru.for_each_evicted(|id| evicted.push(id));
        assert_eq!(evicted.len(), 35);
        assert_eq!(retained_count(&mut lru), 35);
    }
}
