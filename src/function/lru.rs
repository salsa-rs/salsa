use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{hash::FxLinkedHashSet, Id};

use parking_lot::Mutex;

#[derive(Default)]
pub(super) struct Lru {
    capacity: AtomicUsize,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl Lru {
    pub(super) fn record_use(&self, index: Id) -> Option<Id> {
        // Relaxed should be fine, we don't need to synchronize on this.
        let capacity = self.capacity.load(Ordering::Relaxed);

        if capacity == 0 {
            // LRU is disabled
            return None;
        }

        let mut set = self.set.lock();
        set.insert(index);
        if set.len() > capacity {
            return set.pop_front();
        }

        None
    }

    pub(super) fn set_capacity(&self, capacity: usize) {
        // Relaxed should be fine, we don't need to synchronize on this.
        self.capacity.store(capacity, Ordering::Relaxed);

        if capacity == 0 {
            let mut set = self.set.lock();
            *set = FxLinkedHashSet::default();
        }
    }
}
