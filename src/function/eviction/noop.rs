//! No-op eviction policy - cache grows unbounded.
//!
//! This is the default eviction policy when no LRU capacity is specified.

use crate::{Id, Revision, function::EvictionPolicy};

/// No eviction - cache grows unbounded.
pub struct NoopEviction;

impl EvictionPolicy for NoopEviction {
    fn new(_cap: usize) -> Self {
        Self
    }

    #[inline(always)]
    fn admit(&self, _id: Id) {}

    #[inline(always)]
    fn promote(&self, _id: Id) {}

    #[inline(always)]
    fn set_capacity(&mut self, _capacity: usize) {}

    #[inline(always)]
    fn evict(&mut self, _last_verified_at: impl FnMut(Id) -> Option<Revision>) -> Vec<Id> {
        Vec::new()
    }
}
