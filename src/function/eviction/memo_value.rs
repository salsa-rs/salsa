use std::ops::Deref;
use std::sync::Arc;

use arc_swap::ArcSwapOption;

/// Policy-specific storage for a memoized query value.
#[doc(hidden)]
pub trait MemoValue<T>: Send + Sync {
    type Guard<'a>: Deref<Target = T>
    where
        Self: 'a;

    fn new(value: Option<T>) -> Self;
    fn load(&self) -> Option<Self::Guard<'_>>;
    fn is_some(&self) -> bool;
    /// Clears the value, returning whether one was present.
    fn clear(&self) -> bool;

    /// Borrows an inline value. Volatile storage cannot implement this because
    /// its value may be concurrently removed.
    fn borrow_inline(&self) -> Option<&T>;

    /// Acquires an owned handle. Only volatile storage implements this.
    fn load_volatile(&self) -> Option<Arc<T>>;
}

impl<T: Send + Sync> MemoValue<T> for Option<T> {
    type Guard<'a>
        = &'a T
    where
        Self: 'a;

    fn new(value: Option<T>) -> Self {
        value
    }

    fn load(&self) -> Option<Self::Guard<'_>> {
        self.as_ref()
    }

    fn is_some(&self) -> bool {
        Option::is_some(self)
    }

    fn clear(&self) -> bool {
        panic!("inline memo values require exclusive access for eviction")
    }

    fn borrow_inline(&self) -> Option<&T> {
        self.as_ref()
    }

    fn load_volatile(&self) -> Option<Arc<T>> {
        panic!("inline memo values cannot produce volatile handles")
    }
}

impl<T: Send + Sync> MemoValue<T> for ArcSwapOption<T> {
    type Guard<'a>
        = Arc<T>
    where
        Self: 'a;

    fn new(value: Option<T>) -> Self {
        ArcSwapOption::from(value.map(Arc::new))
    }

    fn load(&self) -> Option<Self::Guard<'_>> {
        self.load_full()
    }

    fn is_some(&self) -> bool {
        self.load().is_some()
    }

    fn clear(&self) -> bool {
        self.swap(None).is_some()
    }

    fn borrow_inline(&self) -> Option<&T> {
        panic!("volatile memo values cannot be borrowed")
    }

    fn load_volatile(&self) -> Option<Arc<T>> {
        self.load_full()
    }
}
