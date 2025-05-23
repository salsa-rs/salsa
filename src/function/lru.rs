use std::num::NonZeroUsize;

use crate::hash::FxLinkedHashSet;
use crate::sync::Mutex;
use crate::Id;

pub(super) struct Lru {
    capacity: Option<NonZeroUsize>,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl Lru {
    pub fn new(cap: usize) -> Self {
        Self {
            capacity: NonZeroUsize::new(cap),
            set: Mutex::default(),
        }
    }

    #[inline(always)]
    pub(super) fn record_use(&self, index: Id) {
        if self.capacity.is_some() {
            self.insert(index);
        }
    }

    #[inline(never)]
    fn insert(&self, index: Id) {
        let mut set = self.set.lock();
        set.insert(index);
    }

    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = NonZeroUsize::new(capacity);
        if self.capacity.is_none() {
            self.set.get_mut().clear();
        }
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
