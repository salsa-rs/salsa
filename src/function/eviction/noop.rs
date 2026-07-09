//! No-op eviction policy - cache grows unbounded.
//!
//! This is the default eviction policy when no LRU capacity is specified.

use crate::{Id, function::EvictionPolicy};

/// No eviction - cache grows unbounded.
pub struct NoopEviction;

impl EvictionPolicy for NoopEviction {
    type Value<T: Send + Sync> = Option<T>;

    fn new(_cap: usize) -> Self {
        Self
    }

    #[inline(always)]
    fn set_tuning(&mut self, _capacity: usize) {}

    #[inline(always)]
    fn for_each_evicted(&mut self, _evict: impl FnMut(Id)) {}
}
