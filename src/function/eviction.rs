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

/// Cache eviction policy for memoized function values.
///
/// Implementations control which memoized values are evicted from the cache.
/// The eviction policy is selected at compile time by the `Configuration`
/// trait.
pub trait EvictionPolicy: Send + Sync {
    /// Creates an eviction policy with the given capacity.
    ///
    /// A value of zero disables eviction.
    fn new(capacity: usize) -> Self;

    /// Records that a memoized value was accessed.
    fn record_use(&self, _id: Id) {}

    /// Invokes `evict` for each value selected for eviction.
    ///
    /// Salsa calls this once per revision during `reset_for_new_revision`.
    fn for_each_evicted(&mut self, evict: impl FnMut(Id));
}

/// An eviction policy whose capacity can be changed at runtime.
///
/// This trait is used to conditionally generate the `set_eviction_capacity` method
/// on tracked functions. Only policies that implement this trait will expose
/// runtime capacity changes.
pub trait HasCapacity: EvictionPolicy {
    /// Sets the maximum capacity.
    ///
    /// A value of zero disables eviction.
    fn set_capacity(&mut self, capacity: usize);
}
