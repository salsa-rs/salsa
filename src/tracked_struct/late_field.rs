use std::{cell::UnsafeCell, mem::MaybeUninit, sync::atomic::{AtomicUsize, Ordering}};

use crate::{Revision, Update};

const EMPTY: usize = 0;
const ACQUIRED: usize = 1;
const SET: usize = 2;
const DIRTY: usize = 3;

#[derive(Debug)]
pub struct LateField<T: Update> {
    state: AtomicUsize,
    // Last valid revision of DIRTY state
    old_revision: Option<Revision>,

    data: UnsafeCell<MaybeUninit<T>>
}

unsafe impl<T: Update + Send> Send for LateField<T> {}
unsafe impl<T: Update + Sync> Sync for LateField<T> {}

impl<T: Update> LateField<T> {
    pub fn new() -> LateField<T> {
        LateField { 
            state: AtomicUsize::new(EMPTY), 
            old_revision: None,

            data: UnsafeCell::new(MaybeUninit::uninit()) 
        }
    }

    // Update self, store old revision to probably backdate later
    pub fn maybe_update(&mut self, mut value: Self, maybe_update_inner: unsafe fn(*mut T, T) -> bool, old_revision: Revision) -> bool {
        let old_state = self.state.load(Ordering::Relaxed);
        let new_state = value.state.load(Ordering::Relaxed);
        let t = match (old_state, new_state) {
            (EMPTY, EMPTY) => {
                self.old_revision = None;
                self.state.store(EMPTY, Ordering::Release);

                false
            },
            (EMPTY, SET) => {
                self.old_revision = None;
                self.data = value.data;
                self.state.store(SET, Ordering::Release);

                true
            },
            (DIRTY, SET) => {
                // SAFETY: DIRTY and SET state assumes that data is initialized
                let changed = unsafe {
                    maybe_update_inner(self.data.get_mut().assume_init_mut(), value.data.get_mut().assume_init_read())
                };
                self.state.store(SET, Ordering::Release);

                changed
            },
            (SET, EMPTY) => {
                self.old_revision = Some(old_revision);
                // Save old value to probably backdate later
                self.state.store(DIRTY, Ordering::Release);

                true
            }
            _ => panic!("unexpected state"),
        };

        t
    }

    /// Set new value and returns saved revision if its not updated
    pub fn set_maybe_backdate(&self, value: T) -> Option<Revision> {
        let old_state = self.state.load(Ordering::Relaxed);
        match old_state {
            EMPTY => {},
            DIRTY => {},
            SET => {
                panic!("set on late field called twice")
            },
            ACQUIRED => {
                panic!("concurrent set on late field is not allowed")
            }
            _ => panic!("unexpected state"),
        }
        self.state.compare_exchange(old_state, ACQUIRED, Ordering::Acquire, Ordering::Relaxed).expect("concurrent set on late field is not allowed");
        let updated = if old_state == EMPTY {
            unsafe {
                (&mut *self.data.get()).write(value);
            }
            true
        } else {
            unsafe {
                Update::maybe_update((&mut *self.data.get()).assume_init_mut(), value)
            }
        };

        self.state.store(SET, Ordering::Release);

        if updated {
            None
        } else {
            self.old_revision
        }
    }

    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) != SET {
            return None;
        };

        // SAFETY: we can't move from SET to any other state while we have ref to self
        Some(unsafe {
            (&*self.data.get()).assume_init_ref()
        })
    }
}
