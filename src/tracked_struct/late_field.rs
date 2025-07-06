use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{Revision, Update};

const EMPTY: usize = 0;
const ACQUIRED: usize = 1;
const SET: usize = 2;
const DIRTY: usize = 3;

pub enum UpdateResult {
    // Was set in some revision, then updated to empty, then set to the same value as before
    Backdate(Revision),
    // Set to some new value
    Update,
    // New state is empty or dirty
    Dirty,
}

#[derive(Debug)]
pub struct LateField<T: Update> {
    state: AtomicUsize,
    // Last valid revision of DIRTY state
    old_revision: Option<Revision>,

    data: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T: Update + Send> Send for LateField<T> {}
unsafe impl<T: Update + Sync> Sync for LateField<T> {}

impl<T: Update> Default for LateField<T> {
    fn default() -> LateField<T> {
        LateField {
            state: AtomicUsize::new(EMPTY),
            old_revision: None,

            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

impl<T: Update> LateField<T> {
    pub fn new() -> LateField<T> {
        Self::default()
    }

    // Update self, store old revision if new field is empty, probably backdate later
    pub fn maybe_update(
        &mut self,
        mut value: Self,
        maybe_update_inner: unsafe fn(*mut T, T) -> bool,
        prev_changed_at: Revision,
    ) -> UpdateResult {
        let old_state = self.state.load(Ordering::Relaxed);
        let new_state = value.state.load(Ordering::Relaxed);
        let t = match (old_state, new_state) {
            (EMPTY, EMPTY) => UpdateResult::Dirty,
            (SET, EMPTY) => {
                self.old_revision = Some(prev_changed_at);
                // Save old value to probably backdate later
                self.state.store(DIRTY, Ordering::Release);

                UpdateResult::Dirty
            }
            (EMPTY, SET) => {
                self.old_revision = None;
                self.data = value.data;
                self.state.store(SET, Ordering::Release);

                UpdateResult::Update
            }
            (DIRTY, SET) => {
                // SAFETY: new value in SET state, so its completely valid at this point
                let changed = unsafe {
                    maybe_update_inner(
                        self.data.get_mut().as_mut_ptr(),
                        value.data.get_mut().assume_init_read(),
                    )
                };
                self.state.store(SET, Ordering::Release);

                if changed {
                    UpdateResult::Update
                } else {
                    UpdateResult::Backdate(
                        self.old_revision
                            .take()
                            .expect("dirty value should always have old_revision"),
                    )
                }
            }
            _ => panic!("unexpected state"),
        };

        t
    }

    /// Set new value and returns saved revision if its not updated
    pub fn set_and_maybe_backdate(
        &self,
        value: T,
        maybe_update_inner: unsafe fn(*mut T, T) -> bool,
    ) -> Option<Revision> {
        let old_state = self.state.load(Ordering::Relaxed);
        match old_state {
            EMPTY => {}
            DIRTY => {}
            SET => {
                panic!("set on late field called twice")
            }
            ACQUIRED => {
                panic!("concurrent set on late field is not allowed")
            }
            _ => panic!("unexpected state"),
        }
        self.state
            .compare_exchange(old_state, ACQUIRED, Ordering::Acquire, Ordering::Relaxed)
            .expect("concurrent set on late field is not allowed");
        let updated = if old_state == EMPTY {
            unsafe {
                (*self.data.get()).write(value);
            }
            true
        } else {
            // SAFETY: Only one thread can set state to ACQUIRE
            unsafe { maybe_update_inner((*self.data.get()).as_mut_ptr(), value) }
        };

        self.state.store(SET, Ordering::Release);

        if updated {
            None
        } else {
            debug_assert!(
                self.old_revision.is_some(),
                "dirty value should always have old_revision"
            );
            self.old_revision
        }
    }

    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) != SET {
            return None;
        };

        // SAFETY: we can't move from SET to any other state while we have ref to self
        Some(unsafe { (*self.data.get()).assume_init_ref() })
    }
}
