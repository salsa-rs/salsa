use std::{
    future::Future,
    mem,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use parking_lot::{Condvar, Mutex};

#[doc(hidden)]
pub struct BlockingFuture<T> {
    slot: Arc<Slot<T>>,
}

#[doc(hidden)]
pub struct Promise<T> {
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

    fn wait(&mut self) -> Option<T> {
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

impl<T> Future for BlockingFuture<T> {
    type Output = Option<T>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(self.wait())
    }
}

pub trait PromiseTrait<T> {
    fn fulfil(self, value: T);
}

impl<T> PromiseTrait<T> for Promise<T> {
    fn fulfil(self, value: T) {
        Promise::fulfil(self, value)
    }
}
#[doc(hidden)]
pub trait BlockingFutureTrait<T>: Future<Output = Option<T>> + Sized {
    type Promise: PromiseTrait<T>;
    fn new() -> (Self, Self::Promise);
}

impl<T> BlockingFutureTrait<T> for BlockingFuture<T> {
    type Promise = Promise<T>;
    fn new() -> (BlockingFuture<T>, Promise<T>) {
        BlockingFuture::new()
    }
}

use futures::{channel::oneshot, future::FutureExt};

/// Async variant of BlockingFuture
pub type BlockingAsyncFuture<T> =
    futures::future::Map<oneshot::Receiver<T>, fn(Result<T, oneshot::Canceled>) -> Option<T>>;

impl<T> PromiseTrait<T> for oneshot::Sender<T> {
    fn fulfil(self, value: T) {
        let _ = self.send(value);
    }
}

impl<T> BlockingFutureTrait<T> for BlockingAsyncFuture<T> {
    type Promise = oneshot::Sender<T>;
    fn new() -> (Self, Self::Promise) {
        let (tx, rx) = oneshot::channel();
        (rx.map(|r| r.ok()), tx)
    }
}
