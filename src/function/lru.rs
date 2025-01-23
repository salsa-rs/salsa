use crate::{hash::FxLinkedHashSet, Id};

use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

mod sealed {
    pub trait Sealed {}
}

pub trait LruChoice: sealed::Sealed + Default {
    type LruCtor<T>: Send + Sync
    where
        T: Send + Sync;
    /// Records the `index` into this LRU, returning the index to evict if any.
    fn record_use(&self, index: Id);
    /// Fetches all elements that should be evicted.
    fn to_be_evicted(&self, cb: impl FnMut(Id));
    /// Sets the capacity of the LRU.
    fn set_capacity(&self, capacity: usize);
    fn if_enabled(f: impl FnOnce());

    fn evicted<T>() -> Self::LruCtor<T>
    where
        T: Send + Sync;

    fn is_evicted<T>(it: &Self::LruCtor<T>) -> bool
    where
        T: Send + Sync;

    fn assert_ref<T>(v: &Self::LruCtor<T>) -> &T
    where
        T: Send + Sync;

    fn make_value<T>(v: T) -> Self::LruCtor<T>
    where
        T: Send + Sync;

    fn with_value<T>(v: &Self::LruCtor<T>, cb: impl FnOnce(&T))
    where
        T: Send + Sync;
}

/// An LRU choice that does not evict anything.
#[derive(Default)]
pub struct NoLru;

impl sealed::Sealed for NoLru {}
impl LruChoice for NoLru {
    type LruCtor<T>
        = T
    where
        T: Send + Sync;
    fn record_use(&self, _: Id) {}
    fn to_be_evicted(&self, _: impl FnMut(Id)) {}
    fn set_capacity(&self, _: usize) {}
    fn if_enabled(_: impl FnOnce()) {}

    fn evicted<T>() -> Self::LruCtor<T>
    where
        T: Send + Sync,
    {
        unreachable!("NoLru::evicted should never be called")
    }
    fn is_evicted<T>(_: &Self::LruCtor<T>) -> bool
    where
        T: Send + Sync,
    {
        false
    }
    fn assert_ref<T>(v: &Self::LruCtor<T>) -> &T
    where
        T: Send + Sync,
    {
        v
    }
    fn make_value<T>(v: T) -> Self::LruCtor<T>
    where
        T: Send + Sync,
    {
        v
    }
    fn with_value<T>(v: &Self::LruCtor<T>, cb: impl FnOnce(&T))
    where
        T: Send + Sync,
    {
        cb(v);
    }
}

/// An LRU choice that tracks elements but does not evict on its own.
///
/// The user must manually trigger eviction.
#[derive(Default)]
pub struct Lru {
    capacity: AtomicCell<usize>,
    set: Mutex<FxLinkedHashSet<Id>>,
}

impl sealed::Sealed for Lru {}
impl LruChoice for Lru {
    type LruCtor<T>
        = Option<T>
    where
        T: Send + Sync;
    fn record_use(&self, index: Id) {
        self.set.lock().insert(index);
    }
    fn to_be_evicted(&self, mut cb: impl FnMut(Id)) {
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
    fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity);
        if capacity == 0 {
            *self.set.lock() = FxLinkedHashSet::default();
        }
    }
    fn if_enabled(f: impl FnOnce()) {
        f();
    }

    fn evicted<T>() -> Self::LruCtor<T>
    where
        T: Send + Sync,
    {
        None
    }
    fn is_evicted<T>(it: &Self::LruCtor<T>) -> bool
    where
        T: Send + Sync,
    {
        it.is_none()
    }
    fn assert_ref<T>(v: &Self::LruCtor<T>) -> &T
    where
        T: Send + Sync,
    {
        v.as_ref().unwrap()
    }
    fn make_value<T>(v: T) -> Self::LruCtor<T>
    where
        T: Send + Sync,
    {
        Some(v)
    }
    fn with_value<T>(v: &Self::LruCtor<T>, cb: impl FnOnce(&T))
    where
        T: Send + Sync,
    {
        if let Some(v) = v {
            cb(v);
        }
    }
}
