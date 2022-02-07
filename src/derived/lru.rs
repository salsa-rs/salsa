use crate::hash::FxLinkedHashSet;

use super::DerivedKeyIndex;
use crossbeam_utils::atomic::AtomicCell;
use parking_lot::Mutex;

#[derive(Default)]
pub(super) struct Lru {
    capacity: AtomicCell<usize>,
    set: Mutex<FxLinkedHashSet<DerivedKeyIndex>>,
}

impl Lru {
    pub(super) fn record_use(&self, index: DerivedKeyIndex) -> Option<DerivedKeyIndex> {
        let capacity = self.capacity.load();

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
        self.capacity.store(capacity);

        if capacity == 0 {
            let mut set = self.set.lock();
            *set = FxLinkedHashSet::default();
        }
    }
}
