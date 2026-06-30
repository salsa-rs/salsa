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
    /// Creates a policy from its configured tuning value.
    ///
    /// A value of zero disables eviction.
    fn new(tuning: usize) -> Self;

    /// Records that `id` transitioned from having no cached value to having one.
    fn record_insert(&self, _id: Id) {}

    /// Records that a resident value was used.
    ///
    /// Implementations may treat this as a best-effort hint.
    fn record_use(&self, _id: Id) {}

    /// Changes the policy's tuning value.
    ///
    /// A value of zero disables eviction.
    fn set_tuning(&mut self, tuning: usize);

    /// Processes the start of a new revision.
    ///
    /// The policy may update its state, inspect memo metadata, and evict
    /// resident values through `context`.
    fn start_new_revision(&mut self, context: &mut impl EvictionContext);
}

/// Memo operations available when starting a new revision.
pub trait EvictionContext {
    /// Returns the revision in which `id`'s resident value was last verified.
    ///
    /// Returns `None` if the memo no longer contains a resident value.
    fn last_verified_at(&mut self, id: Id) -> Option<Revision>;

    /// Evicts `id`'s cached value while retaining its memo metadata.
    fn evict_value(&mut self, id: Id);
}

/// Marker trait for eviction policies whose tuning can be changed at runtime.
///
/// This trait is used to conditionally generate the `set_lru_capacity` method
/// on tracked functions. Only policies that implement this trait will expose
/// runtime tuning.
pub trait HasCapacity: EvictionPolicy {}
