#[cfg(loom)]
pub use loom::{cell, thread, thread_local};

/// A helper macro to work around the fact that most loom types are not `const` constructable.
#[doc(hidden)]
#[macro_export]
macro_rules! __maybe_lazy_static {
    (static $name:ident: $t:ty = $init:expr $(;)?) => {
        #[cfg(loom)]
        loom::lazy_static! { static ref $name: $t = $init; }

        #[cfg(not(loom))]
        static $name: $t = $init;
    };
}

pub(crate) use crate::__maybe_lazy_static as maybe_lazy_static;

/// A polyfill for `Atomic*::get_mut`, which loom does not support.
pub trait AtomicMut<T> {
    fn read_mut(&mut self) -> T;
    fn write_mut(&mut self, value: T);
}

#[cfg(loom)]
pub mod sync {
    pub use super::AtomicMut;
    pub use loom::sync::*;

    /// A wrapper around loom's `Mutex` to mirror parking-lot's API.
    #[derive(Default, Debug)]
    pub struct Mutex<T>(loom::sync::Mutex<T>);

    impl<T> Mutex<T> {
        pub fn new(value: T) -> Mutex<T> {
            Mutex(loom::sync::Mutex::new(value))
        }

        pub fn lock(&self) -> MutexGuard<'_, T> {
            self.0.lock().unwrap()
        }

        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap()
        }
    }

    /// A wrapper around loom's `RwLock` to mirror parking-lot's API.
    #[derive(Default, Debug)]
    pub struct RwLock<T>(loom::sync::RwLock<T>);

    impl<T> RwLock<T> {
        pub fn read(&self) -> RwLockReadGuard<'_, T> {
            self.0.read().unwrap()
        }

        pub fn write(&self) -> RwLockWriteGuard<'_, T> {
            self.0.write().unwrap()
        }

        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap()
        }
    }

    /// A wrapper around loom's `Condvar` to mirror parking-lot's API.
    #[derive(Default, Debug)]
    pub struct Condvar(loom::sync::Condvar);

    impl Condvar {
        // We cannot match parking-lot identically because loom's version takes ownership of the `MutexGuard`.
        pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
            self.0.wait(guard).unwrap()
        }

        pub fn notify_one(&self) {
            self.0.notify_one();
        }

        pub fn notify_all(&self) {
            self.0.notify_all();
        }
    }

    use loom::cell::UnsafeCell;
    use std::mem::MaybeUninit;

    /// A polyfill for `std::sync::OnceLock`.
    pub struct OnceLock<T>(Mutex<bool>, UnsafeCell<MaybeUninit<T>>);

    impl<T> OnceLock<T> {
        pub fn new() -> OnceLock<T> {
            OnceLock(Mutex::new(false), UnsafeCell::new(MaybeUninit::uninit()))
        }

        pub fn get(&self) -> Option<&T> {
            let initialized = self.0.lock();
            if *initialized {
                // SAFETY: The value is initialized and write-once.
                Some(self.1.with(|ptr| unsafe { (*ptr).assume_init_ref() }))
            } else {
                None
            }
        }

        pub fn set(&self, value: T) -> Result<(), T> {
            let mut initialized = self.0.lock();
            if *initialized {
                Err(value)
            } else {
                self.1.with_mut(|ptr| {
                    // SAFETY: We hold the lock.
                    unsafe { ptr.write(MaybeUninit::new(value)) }
                });
                *initialized = true;
                Ok(())
            }
        }
    }

    impl<T> From<T> for OnceLock<T> {
        fn from(value: T) -> OnceLock<T> {
            OnceLock(Mutex::new(true), UnsafeCell::new(MaybeUninit::new(value)))
        }
    }

    // SAFETY: Mirroring `std::sync::OnceLock`.
    unsafe impl<T: Send> Send for OnceLock<T> {}
    // SAFETY: Mirroring `std::sync::OnceLock`.
    unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}

    /// Extend `Atomic*` with mutable accessors.
    macro_rules! impl_loom_atomic_mut {
        ($($atomic_ty:ident $(<$generic:ident>)? => $ty:ty),*) => {$(
            impl $(<$generic>)? super::AtomicMut<$ty> for atomic::$atomic_ty $(<$generic>)? {
                fn read_mut(&mut self) -> $ty {
                    self.load(atomic::Ordering::Relaxed)
                }

                fn write_mut(&mut self, value: $ty) {
                    self.store(value, atomic::Ordering::Relaxed)
                }
            }
        )*};
    }

    impl_loom_atomic_mut! { AtomicBool => bool, AtomicUsize => usize, AtomicPtr<T> => *mut T }
}

#[cfg(not(loom))]
pub use std::{thread, thread_local};

#[cfg(not(loom))]
pub mod cell {
    pub use std::cell::*;

    #[derive(Debug)]
    pub(crate) struct UnsafeCell<T>(core::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        pub const fn new(data: T) -> UnsafeCell<T> {
            UnsafeCell(core::cell::UnsafeCell::new(data))
        }

        #[inline(always)]
        pub fn with<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*const T) -> R,
        {
            f(self.0.get())
        }

        #[inline(always)]
        pub fn with_mut<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*mut T) -> R,
        {
            f(self.0.get())
        }

        #[inline(always)]
        pub(crate) fn get_mut(&self) -> MutPtr<T> {
            MutPtr(self.0.get())
        }
    }

    #[derive(Debug)]
    pub(crate) struct MutPtr<T: ?Sized>(*mut T);

    impl<T: ?Sized> MutPtr<T> {
        #[inline(always)]
        pub fn with<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*mut T) -> R,
        {
            f(self.0)
        }
    }
}

#[cfg(not(loom))]
pub mod sync {
    pub use super::AtomicMut;
    pub use parking_lot::{Mutex, MutexGuard, RwLock};
    pub use std::sync::*;

    pub mod atomic {
        pub use portable_atomic::AtomicU64;
        pub use std::sync::atomic::*;
    }

    /// A wrapper around parking-lot's `Condvar` to mirror loom's API.
    #[derive(Default, Debug)]
    pub struct Condvar(parking_lot::Condvar);

    impl Condvar {
        pub fn wait<'a, T>(&self, mut guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
            self.0.wait(&mut guard);
            guard
        }

        pub fn notify_one(&self) {
            self.0.notify_one();
        }

        pub fn notify_all(&self) {
            self.0.notify_all();
        }
    }
}

/// Extend `Atomic*` with mutable accessors.
macro_rules! impl_std_atomic_mut {
    ($($atomic_ty:ident $(<$generic:ident>)? => $ty:ty),*) => {$(
        #[cfg(not(loom))]
        impl $(<$generic>)? AtomicMut<$ty> for sync::atomic::$atomic_ty $(<$generic>)? {
            fn read_mut(&mut self) -> $ty {
                *self.get_mut()
            }

            fn write_mut(&mut self, value: $ty) {
                *self.get_mut() = value;
            }
        }
    )*};
}

impl_std_atomic_mut! { AtomicBool => bool, AtomicUsize => usize, AtomicPtr<T> => *mut T }
