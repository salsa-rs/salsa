use std::num::NonZeroUsize;

use crate::{hash::FxLinkedHashSet, Id};

use parking_lot::Mutex;

pub(super) struct Lru {
    capacity: Option<NonZeroUsize>,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl Lru {
    pub fn new(cap: usize) -> Self {
        Self {
            capacity: NonZeroUsize::new(cap),
            set: Mutex::new(FxLinkedHashSet::default()),
        }
    }

    pub(super) fn record_use(&self, index: Id) {
        if self.capacity.is_none() {
            // LRU is disabled
            return;
        }

        let mut set = self.set.lock();
        set.insert(index);
    }

    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = NonZeroUsize::new(capacity);
    }

    pub(super) fn for_each_evicted(&mut self, mut cb: impl FnMut(Id)) {
        let Some(cap) = self.capacity else {
            return;
        };
        let set = self.set.get_mut();
        while set.len() > cap.get() {
            if let Some(id) = set.pop_front() {
                cb(id);
            }
        }
    }
}
