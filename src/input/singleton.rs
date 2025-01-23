use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

use crate::{id::FromId, Id};

mod sealed {
    pub trait Sealed {}
}

pub trait SingletonChoice: sealed::Sealed + Default {
    fn with_lock(&self, cb: impl FnOnce() -> Id) -> Id;
    fn index(&self) -> Option<Id>;
}

pub struct Singleton {
    index: AtomicCell<Option<Id>>,
    lock: Mutex<()>,
}
impl sealed::Sealed for Singleton {}
impl SingletonChoice for Singleton {
    fn with_lock(&self, cb: impl FnOnce() -> Id) -> Id {
        let _guard = self.lock.lock();
        if self.index.load().is_some() {
            panic!("singleton struct may not be duplicated");
        }
        let id = cb();
        self.index.store(Some(id));
        drop(_guard);
        id
    }

    fn index(&self) -> Option<Id> {
        self.index.load().map(FromId::from_id)
    }
}

impl Default for Singleton {
    fn default() -> Self {
        Self {
            index: AtomicCell::new(None),
            lock: Default::default(),
        }
    }
}
#[derive(Default)]
pub struct NotSingleton;
impl sealed::Sealed for NotSingleton {}
impl SingletonChoice for NotSingleton {
    fn with_lock(&self, cb: impl FnOnce() -> Id) -> Id {
        cb()
    }
    fn index(&self) -> Option<Id> {
        None
    }
}
