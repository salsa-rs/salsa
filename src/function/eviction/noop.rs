//! No-op eviction policy - cache grows unbounded.
//!
//! This is the default eviction policy when eviction is not configured.

use crate::{Id, function::EvictionPolicy};

/// No eviction - cache grows unbounded.
pub struct NoopEviction;

impl EvictionPolicy for NoopEviction {
    fn new(_cap: usize) -> Self {
        Self
    }

    #[inline(always)]
    fn for_each_evicted(&mut self, _evict: impl FnMut(Id)) {}
}
