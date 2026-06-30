//! No-op eviction policy - cache grows unbounded.
//!
//! This is the default eviction policy when no LRU capacity is specified.

use crate::function::{EvictionContext, EvictionPolicy};

/// No eviction - cache grows unbounded.
pub struct NoopEviction;

impl EvictionPolicy for NoopEviction {
    fn new(_cap: usize) -> Self {
        Self
    }

    #[inline(always)]
    fn set_tuning(&mut self, _capacity: usize) {}

    #[inline(always)]
    fn start_new_revision(&mut self, _context: &mut impl EvictionContext) {}
}
