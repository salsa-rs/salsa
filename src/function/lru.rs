use crate::{hash::FxLinkedHashSet, Id};

use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

#[derive(Default)]
pub(super) struct Lru {
    capacity: AtomicCell<usize>,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl Lru {
    pub(super) fn record_use(&self, index: Id) {
        let capacity = self.capacity.load();

        if capacity == 0 {
            // LRU is disabled
            return;
        }

        let mut set = self.set.lock();
        set.insert(index);
    }

    pub(super) fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity);
    }

    pub(super) fn to_be_evicted(&self, mut cb: impl FnMut(Id)) {
        let mut set = self.set.lock();
        let cap = self.capacity.load();
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
