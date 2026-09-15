//! Pluggable cache eviction strategies for memoized function values.
//!
//! This module provides the [`EvictionPolicy`] trait that allows different
//! eviction strategies to be used for salsa tracked functions.

mod lru;
mod noop;

pub use lru::Lru;
pub use noop::NoopEviction;

use crate::Id;

/// Trait for cache eviction strategies.
///
/// Implementations control when memoized values are evicted from the cache.
/// The eviction policy is selected at compile time via the `Configuration` trait.
pub trait EvictionPolicy: Send + Sync {
    /// Create a new eviction policy with the given capacity.
    fn new(capacity: usize) -> Self;

    /// Record that an item was accessed.
    fn record_use(&self, id: Id);

    /// Set the maximum capacity.
    fn set_capacity(&mut self, capacity: usize);

    /// Whether [`Self::set_capacity`] does anything for this policy.
    ///
    /// [`HasCapacity`] answers the same question, but only at compile time, which
    /// is useless once the ingredient is behind `dyn Ingredient`. Callers that
    /// reach an ingredient dynamically (by name, or by walking the whole
    /// registry) need to tell "capacity applied" from "this query has no LRU at
    /// all", and a no-op `set_capacity` cannot report that difference.
    fn has_tunable_capacity(&self) -> bool {
        false
    }

    /// Iterate over items that should be evicted.
    ///
    /// Called once per revision during `reset_for_new_revision`.
    /// The callback `cb` should be invoked for each item to evict.
    fn for_each_evicted(&mut self, cb: impl FnMut(Id));
}

/// Marker trait for eviction policies that have a configurable capacity.
///
/// This trait is used to conditionally generate the `set_lru_capacity` method
/// on tracked functions. Only policies that implement this trait will expose
/// runtime capacity configuration.
pub trait HasCapacity: EvictionPolicy {}
