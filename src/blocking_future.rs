use parking_lot::{Condvar, Mutex};
use std::mem;
use std::sync::Arc;

pub(crate) struct BlockingFuture<T> {
    slot: Arc<Slot<T>>,
}

pub(crate) struct Promise<T> {
    fulfilled: bool,
    slot: Arc<Slot<T>>,
}

impl<T> BlockingFuture<T> {
    pub(crate) fn new() -> (BlockingFuture<T>, Promise<T>) {
        let future = BlockingFuture {
            slot: Default::default(),
        };
        let promise = Promise {
            fulfilled: false,
            slot: Arc::clone(&future.slot),
        };
        (future, promise)
    }

    pub(crate) fn wait(self) -> Option<T> {
        let mut guard = self.slot.lock.lock();
        if guard.is_empty() {
            // parking_lot guarantees absence of spurious wake ups
            self.slot.cvar.wait(&mut guard);
        }
        match mem::replace(&mut *guard, State::Dead) {
            State::Empty => unreachable!(),
            State::Full(it) => Some(it),
            State::Dead => None,
        }
    }
}

impl<T> Promise<T> {
    pub(crate) fn fulfil(mut self, value: T) {
        self.fulfilled = true;
        self.transition(State::Full(value));
    }
    fn transition(&mut self, new_state: State<T>) {
        let mut guard = self.slot.lock.lock();
        *guard = new_state;
        self.slot.cvar.notify_one();
    }
}

impl<T> Drop for Promise<T> {
    fn drop(&mut self) {
        if !self.fulfilled {
            self.transition(State::Dead);
        }
    }
}

struct Slot<T> {
    lock: Mutex<State<T>>,
    cvar: Condvar,
}

impl<T> Default for Slot<T> {
    fn default() -> Slot<T> {
        Slot {
            lock: Mutex::new(State::Empty),
            cvar: Condvar::new(),
        }
    }
}

enum State<T> {
    Empty,
    Full(T),
    Dead,
}

impl<T> State<T> {
    fn is_empty(&self) -> bool {
        matches!(self, State::Empty)
    }
}
