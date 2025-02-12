use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{hash::FxLinkedHashSet, Id};

use parking_lot::Mutex;

#[derive(Default)]
pub(super) struct Lru {
    capacity: AtomicUsize,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl Lru {
    pub(super) fn record_use(&self, index: Id) {
        // Relaxed should be fine, we don't need to synchronize on this.
        let capacity = self.capacity.load(Ordering::Relaxed);
        if capacity == 0 {
            // LRU is disabled
            return;
        }

        let mut set = self.set.lock();
        set.insert(index);
    }

    pub(super) fn set_capacity(&self, capacity: usize) {
        // Relaxed should be fine, we don't need to synchronize on this.
        self.capacity.store(capacity, Ordering::Relaxed);
    }

    pub(super) fn for_each_evicted(&self, mut cb: impl FnMut(Id)) {
        let mut set = self.set.lock();
        // Relaxed should be fine, we don't need to synchronize on this.
        let cap = self.capacity.load(Ordering::Relaxed);
        if set.len() <= cap || cap == 0 {
            return;
        }
        while let Some(id) = set.pop_front() {
            cb(id);
            if set.len() <= cap {
                break;
            }
        }
    }
}
