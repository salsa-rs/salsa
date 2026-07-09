//! Pluggable cache eviction strategies for memoized function values.
//!
//! This module provides the [`EvictionPolicy`] trait that allows different
//! eviction strategies to be used for salsa tracked functions.

mod lru;
mod memo_value;
mod noop;
mod sieve;
mod volatile;

pub use lru::Lru;
pub use memo_value::MemoValue;
pub use noop::NoopEviction;
pub use sieve::Sieve;
pub use volatile::Volatile;

use crate::Id;

/// Trait for cache eviction strategies.
///
/// Implementations control when memoized values are evicted from the cache.
/// The eviction policy is selected at compile time via the `Configuration` trait.
pub trait EvictionPolicy: Send + Sync {
    /// Storage used for memoized values under this policy.
    #[doc(hidden)]
    type Value<T: Send + Sync>: MemoValue<T>;

    /// Whether the memo allocation contains the output value itself.
    #[doc(hidden)]
    const STORES_VALUE_INLINE: bool = true;

    /// Whether this policy can retire memo values within a revision.
    const RETIRES_VALUES: bool = false;

    /// Creates a policy from its configured tuning value.
    ///
    /// A value of zero disables eviction.
    fn new(tuning: usize) -> Self;

    /// Records that a value was used.
    ///
    /// Implementations may treat this as a best-effort hint.
    fn record_use(&self, _id: Id) {}

    /// Records that an item was accessed through an owned volatile handle.
    ///
    /// Returns an item whose cached value should be retired after the caller
    /// has acquired the handle.
    fn record_volatile_use(&self, id: Id) -> Option<Id> {
        self.record_use(id);
        None
    }

    /// Changes the policy's tuning value.
    ///
    /// A value of zero disables eviction.
    fn set_tuning(&mut self, tuning: usize);

    /// Invokes `evict` for each value that should be evicted.
    ///
    /// Called once per revision. Implementations may also perform any
    /// revision-boundary maintenance before returning.
    fn for_each_evicted(&mut self, evict: impl FnMut(Id));
}

/// Marker trait for eviction policies whose tuning can be changed at runtime.
///
/// This trait is used to conditionally generate the `set_lru_capacity` method
/// on tracked functions. Only policies that implement this trait will expose
/// runtime tuning.
pub trait HasCapacity: EvictionPolicy {}
