use std::hash::Hash;
use std::num::NonZeroUsize;

use crate::durability::Durability;
use crate::function::VerifyResult;
use crate::plumbing::ZalsaLocal;
use crate::zalsa::Zalsa;
use crate::{DatabaseKeyIndex, Id, Revision};

use super::{EvictionPolicy, InternExisting, InternMissing};
use crate::interned::{Configuration, HashEqLike};

/// Compile-time opt-out from interned eviction.
pub struct NoopEviction;

/// Stable per-value state for an interned ingredient without eviction.
pub struct NoopEntry {
    id: Id,
}

impl EvictionPolicy for NoopEviction {
    type Shard = ();
    type Entry = NoopEntry;
    type Durability = ();

    fn new(_revisions: NonZeroUsize) -> Self {
        Self
    }

    fn new_entry(id: Id, _last_interned_at: Revision) -> Self::Entry {
        NoopEntry { id }
    }

    fn new_durability(_durability: Durability) -> Self::Durability {}

    #[inline(always)]
    fn initial_metadata(
        _zalsa_local: &ZalsaLocal,
        _current_revision: Revision,
    ) -> (Durability, Revision) {
        (Durability::MAX, Revision::max())
    }

    unsafe fn id(entry: &Self::Entry) -> Id {
        entry.id
    }

    unsafe fn serialized_metadata(
        _entry: &Self::Entry,
        _durability: &Self::Durability,
    ) -> (Durability, Revision) {
        (Durability::MAX, Revision::max())
    }

    #[inline(always)]
    fn record_revision(&self, _revision: Revision) {}

    #[inline(always)]
    unsafe fn intern_existing(&self, existing: InternExisting<'_, Self>) -> Id {
        // SAFETY: Guaranteed by the caller.
        unsafe { (*existing.entry).id }
    }

    #[inline(always)]
    fn intern_missing<'db, C, Key, Assemble>(
        &self,
        missing: InternMissing<'_, 'db, C, Key, Assemble>,
    ) -> Id
    where
        C: Configuration<Eviction = Self>,
        Key: Hash,
        C::Fields<'db>: HashEqLike<Key>,
        Assemble: FnOnce(Id, Key) -> C::Fields<'db>,
    {
        missing.intern_new()
    }

    #[inline(always)]
    unsafe fn insert_entry(
        &self,
        _shard: &mut Self::Shard,
        _entry: *const Self::Entry,
        _durability: &Self::Durability,
    ) {
    }

    #[inline(always)]
    unsafe fn report_tracked_read(
        &self,
        _zalsa_local: &ZalsaLocal,
        _index: DatabaseKeyIndex,
        _current_revision: Revision,
        _durability: &Self::Durability,
    ) {
    }

    #[inline(always)]
    unsafe fn is_valid(
        &self,
        _zalsa: &Zalsa,
        _entry: &Self::Entry,
        _durability: &Self::Durability,
    ) -> bool {
        true
    }

    #[inline(always)]
    unsafe fn maybe_changed_after(
        &self,
        _zalsa: &Zalsa,
        _index: DatabaseKeyIndex,
        _input: Id,
        _current_revision: Revision,
        _entry: &Self::Entry,
    ) -> VerifyResult {
        VerifyResult::unchanged()
    }

    #[inline(always)]
    fn can_reuse(&self) -> bool {
        false
    }
}
