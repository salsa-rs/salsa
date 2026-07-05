//! Pluggable cache eviction strategies for memoized function values.
//!
//! This module provides the [`EvictionPolicy`] trait that allows different
//! eviction strategies to be used for salsa tracked functions.

mod lru;
mod noop;
mod sieve;

pub use lru::Lru;
pub use noop::NoopEviction;
pub use sieve::Sieve;

use crate::Id;

/// Trait for cache eviction strategies.
///
/// Implementations control when memoized values are evicted from the cache.
/// The eviction policy is selected at compile time via the `Configuration` trait.
pub trait EvictionPolicy: Send + Sync {
    /// Creates a policy with the configured capacity.
    ///
    /// A value of zero disables eviction.
    fn new(capacity: usize) -> Self;

    /// Records that a value was used.
    ///
    /// Implementations may treat this as a best-effort hint.
    fn record_use(&self, _id: Id) {}

    /// Invokes `evict` for each value that should be evicted.
    ///
    /// Called once per revision. Implementations may also perform any
    /// revision-boundary maintenance before returning.
    fn for_each_evicted(&mut self, evict: impl FnMut(Id));
}

/// An eviction policy whose capacity can be changed at runtime.
///
/// This trait is used to conditionally generate the `set_eviction_capacity` method
/// on tracked functions. Only policies that implement this trait will expose
/// runtime capacity changes.
pub trait HasCapacity: EvictionPolicy {
    /// Changes the policy's capacity.
    ///
    /// A value of zero disables eviction.
    fn set_capacity(&mut self, capacity: usize);
}
