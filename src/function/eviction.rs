//! Pluggable cache eviction strategies for memoized function values.
//!
//! This module provides the [`EvictionPolicy`] trait that allows different
//! eviction strategies to be used for salsa tracked functions.

mod lru;
mod noop;

pub use lru::Lru;
pub use noop::NoopEviction;

use crate::{Id, Revision};

/// Trait for cache eviction strategies.
///
/// Implementations control when memoized values are evicted from the cache.
/// The eviction policy is selected at compile time via the `Configuration` trait.
pub trait EvictionPolicy: Send + Sync {
    /// Create a new eviction policy with the configured `lru` value.
    fn new(value: usize) -> Self;

    /// Record that an item acquired a memoized value that may need to be evicted.
    fn admit(&self, id: Id);

    /// Record that an item was accessed.
    ///
    /// Implementations may treat this as a best-effort hint.
    fn promote(&self, id: Id);

    /// Set the policy's runtime threshold. Zero disables eviction.
    fn set_capacity(&mut self, value: usize);

    /// Return items that should be evicted.
    ///
    /// Called once per revision during `reset_for_new_revision`.
    /// `last_verified_at` returns the last revision in which an item's memoized
    /// value was verified, or `None` if it no longer has a value.
    fn evict(&mut self, last_verified_at: impl FnMut(Id) -> Option<Revision>) -> Vec<Id>;
}

/// Marker trait for eviction policies that have a configurable threshold.
///
/// This trait is used to conditionally generate the `set_lru_capacity` method
/// on tracked functions. Only policies that implement this trait will expose
/// runtime eviction configuration.
pub trait HasCapacity: EvictionPolicy {}
