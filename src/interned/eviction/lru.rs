use std::cell::UnsafeCell;
use std::hash::{BuildHasher, Hash};
use std::num::NonZeroUsize;

use intrusive_collections::{LinkedList, LinkedListLink, UnsafeRef, intrusive_adapter};
use smallvec::SmallVec;

use crate::durability::Durability;
use crate::function::VerifyResult;
use crate::plumbing::ZalsaLocal;
use crate::revision::AtomicRevision;
use crate::sync::Mutex;
use crate::zalsa::Zalsa;
use crate::{DatabaseKeyIndex, Event, EventKind, Id, Revision};

use super::{EvictionPolicy, InternExisting, InternMissing};
use crate::interned::{Configuration, DEFAULT_REVISIONS, HashEqLike, Value, ValueKey};

/// Type-erased state stored in the intrusive LRU list.
pub struct LruEntry {
    /// Intrusive links, accessed only while holding the owning shard lock.
    link: LinkedListLink,

    /// Metadata used to decide whether and how this slot can be reused.
    ///
    /// This may only be accessed while holding the owning shard lock, or with exclusive access to
    /// the database.
    metadata: UnsafeCell<EntryMetadata>,
}

/// Metadata read by the type-erased LRU scan.
///
/// Durability is deliberately not stored here: it determines whether an entry is added to the
/// list, but is not needed once the scan begins. Keeping it on `Value` also lets Rust place the
/// byte in outer padding instead of growing the aligned `LruEntry`.
#[derive(Clone, Copy)]
struct EntryMetadata {
    /// The interned ID for this value.
    ///
    /// Storing this on the value itself is necessary to identify slots
    /// from the LRU list, as well as keep track of the generation.
    ///
    /// Values that are reused increment the ID generation, as if they had
    /// allocated a new slot. This eliminates the need for dependency edges
    /// on queries that *read* from an interned value, as any memos dependent
    /// on the previous value will not match the new ID.
    ///
    /// However, reusing a slot invalidates the previous ID, so queries that
    /// *create* a reusable interned value record dependency edges to ensure the
    /// value is re-interned with a new ID.
    id: Id,

    /// The revision the value was most-recently interned in.
    last_interned_at: Revision,
}

struct ReusableSlot {
    entry: *const LruEntry,
    old_id: Id,
    new_id: Id,
}

intrusive_adapter!(LruEntryAdapter = UnsafeRef<LruEntry>: LruEntry { link => LinkedListLink });

/// LRU eviction for interned values.
pub struct Lru {
    revision_queue: RevisionQueue,
}

/// Selects the reusable or immortal LRU representation at compile time.
#[doc(hidden)]
pub struct LruSelector<const IMMORTAL: bool>;

/// Maps an LRU revision configuration to its eviction policy.
#[doc(hidden)]
pub trait SelectLru {
    type Eviction: EvictionPolicy;
}

impl SelectLru for LruSelector<false> {
    type Eviction = Lru;
}

impl SelectLru for LruSelector<true> {
    type Eviction = ImmortalLru;
}

/// LRU storage without slot reuse, selected for `revisions = usize::MAX`.
#[doc(hidden)]
pub struct ImmortalLru {
    _lru: Lru,
}

/// Per-shard LRU state.
#[derive(Default)]
pub struct LruShard {
    lru: LinkedList<LruEntryAdapter>,
}

// SAFETY: `LruShard` is only accessed through its ingredient shard mutex. Its LRU contains
// pointers to live, stable values, and those pointers and links are accessed only while holding
// that mutex.
unsafe impl Send for LruShard {}

impl EvictionPolicy for Lru {
    const CAN_REUSE: bool = true;

    type Shard = LruShard;
    type Entry = LruEntry;
    type Durability = UnsafeCell<Durability>;

    fn new(revisions: NonZeroUsize) -> Self {
        debug_assert_ne!(revisions, IMMORTAL);
        Self {
            revision_queue: RevisionQueue::new(revisions),
        }
    }

    fn new_entry(id: Id, last_interned_at: Revision) -> Self::Entry {
        LruEntry {
            link: LinkedListLink::new(),
            metadata: UnsafeCell::new(EntryMetadata {
                id,
                last_interned_at,
            }),
        }
    }

    fn new_durability(durability: Durability) -> Self::Durability {
        UnsafeCell::new(durability)
    }

    fn initial_metadata(
        zalsa_local: &ZalsaLocal,
        current_revision: Revision,
    ) -> (Durability, Revision) {
        zalsa_local
            .active_query()
            .map(|(_, stamp)| (stamp.durability, current_revision))
            // If there is no active query this durability does not actually matter.
            // `last_interned_at` needs to be `Revision::MAX`, see the
            // `intern_access_in_different_revision` test.
            .unwrap_or((Durability::MAX, Revision::max()))
    }

    unsafe fn id(entry: &Self::Entry) -> Id {
        // SAFETY: Guaranteed by the caller.
        unsafe { (*entry.metadata.get()).id }
    }

    unsafe fn serialized_metadata(
        entry: &Self::Entry,
        durability: &Self::Durability,
    ) -> (Durability, Revision) {
        // SAFETY: Guaranteed by the caller.
        unsafe { (*durability.get(), (*entry.metadata.get()).last_interned_at) }
    }

    #[inline(always)]
    fn record_revision(&self, revision: Revision) {
        self.revision_queue.record(revision);
    }

    #[inline(always)]
    unsafe fn intern_existing(&self, existing: InternExisting<'_, Self>) -> Id {
        let InternExisting {
            zalsa,
            zalsa_local,
            index,
            current_revision,
            entry,
            durability,
            shard,
        } = existing;

        // SAFETY: Guaranteed by the caller.
        let (metadata, durability) =
            unsafe { (&mut *(*entry).metadata.get(), &mut *durability.get()) };

        if metadata.last_interned_at < current_revision {
            metadata.last_interned_at = current_revision;

            zalsa.event(&|| {
                Event::new(EventKind::DidValidateInternedValue {
                    key: index,
                    revision: current_revision,
                })
            });

            if self.is_reusable(*durability) {
                // SAFETY: The caller holds the shard lock and this reusable entry is in the LRU.
                unsafe { shard.lru.cursor_mut_from_ptr(&*entry).remove() };
                // SAFETY: The caller guarantees that `entry` points to a live value in this shard.
                unsafe { shard.lru.push_front(UnsafeRef::from_raw(entry)) };
            }
        }

        if let Some((_, stamp)) = zalsa_local.active_query() {
            let was_reusable = self.is_reusable(*durability);
            *durability = std::cmp::max(*durability, stamp.durability);

            if was_reusable && !self.is_reusable(*durability) {
                // SAFETY: The caller holds the shard lock and this formerly reusable entry is in the LRU.
                unsafe { shard.lru.cursor_mut_from_ptr(&*entry).remove() };
            }
        }

        self.report_tracked_read_value(zalsa_local, index, current_revision, *durability);

        metadata.id
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
        if !self.revision_queue.is_primed() {
            return missing.intern_new();
        }

        // SAFETY: The caller holds this ingredient's shard lock.
        let Some(slot) =
            (unsafe { self.find_reusable_slot(missing.current_revision, &mut *missing.shard) })
        else {
            return missing.intern_new();
        };

        let InternMissing {
            ingredient,
            zalsa,
            zalsa_local,
            key,
            assemble,
            key_map,
            shard,
            shard_index: _,
            hash,
            current_revision,
        } = missing;

        // SAFETY: The LRU only contains entries derived from live values in this shard.
        let value = unsafe { value_from_entry_ptr::<C>(slot.entry) };

        let (durability, last_interned_at) = Self::initial_metadata(zalsa_local, current_revision);

        // Assemble and hash the replacement before mutating the existing slot. Both operations
        // can invoke user code and panic.
        // SAFETY: `from_internal_data` restores the correct lifetime before access.
        let new_fields = unsafe { ingredient.to_internal_data(assemble(slot.new_id, key)) };
        // SAFETY: We hold the lock for the shard containing the value.
        let old_hash = ingredient.hasher.hash_one(unsafe { &*value.fields.get() });
        let index = ingredient.database_key_index(slot.new_id);

        self.report_tracked_read_value(zalsa_local, index, current_revision, durability);

        // Reserve space while the old slot is intact: hashing may panic during rehashing.
        // SAFETY: The caller holds the lock for every value passed to `hasher`.
        let hasher = |value: &ValueKey| unsafe { ingredient.value_hash(value.value::<C>()) };
        key_map.reserve(1, hasher);

        // SAFETY: The stale, reusable entry is in this shard's LRU.
        unsafe { shard.lru.cursor_mut_from_ptr(&value.eviction).remove() };

        let value_key = ValueKey::new(value);
        key_map
            .find_entry(old_hash, |found_value| found_value.0 == value_key.0)
            .unwrap_or_else(|_| panic!("interned value in LRU so must be in key_map"))
            .remove();

        // SAFETY: A stale reusable value has no outstanding references to its fields or memos.
        let old_fields = unsafe { std::mem::replace(&mut *value.fields.get(), new_fields) };
        // SAFETY: We still hold the shard lock.
        unsafe {
            *value.eviction.metadata.get() = EntryMetadata {
                id: slot.new_id,
                last_interned_at,
            };
            *value.durability.get() = durability;
        }

        key_map.insert_unique(hash, value_key, hasher);

        if self.is_reusable(durability) {
            // SAFETY: The value is live and the entry pointer retains its enclosing provenance.
            unsafe { shard.lru.push_front(UnsafeRef::from_raw(slot.entry)) };
        }

        // SAFETY: A stale reusable value has no outstanding references to its memos.
        let memo_table = unsafe { &mut *value.memos.get() };
        // SAFETY: The memo table belongs to the value allocated by this ingredient.
        unsafe { ingredient.clear_memos(zalsa, memo_table, slot.old_id) };

        drop(old_fields);

        zalsa.event(&|| {
            Event::new(EventKind::DidReuseInternedValue {
                key: index,
                revision: current_revision,
            })
        });

        slot.new_id
    }

    unsafe fn insert_entry(
        &self,
        shard: &mut Self::Shard,
        entry: *const Self::Entry,
        durability: &Self::Durability,
    ) {
        // SAFETY: Guaranteed by the caller.
        let durability = unsafe { *durability.get() };
        if self.is_reusable(durability) {
            // SAFETY: The caller guarantees that `entry` points to a live value in this shard.
            unsafe { shard.lru.push_front(UnsafeRef::from_raw(entry)) };
        }
    }

    unsafe fn report_tracked_read(
        &self,
        zalsa_local: &ZalsaLocal,
        index: DatabaseKeyIndex,
        current_revision: Revision,
        durability: &Self::Durability,
    ) {
        // SAFETY: Guaranteed by the caller.
        let durability = unsafe { *durability.get() };
        self.report_tracked_read_value(zalsa_local, index, current_revision, durability);
    }

    unsafe fn is_valid(
        &self,
        zalsa: &Zalsa,
        entry: &Self::Entry,
        durability: &Self::Durability,
    ) -> bool {
        // SAFETY: Guaranteed by the caller.
        let durability = unsafe { *durability.get() };
        if !self.is_reusable(durability) {
            return true;
        }

        // SAFETY: Guaranteed by the caller.
        let last_interned_at = unsafe { (*entry.metadata.get()).last_interned_at };
        last_interned_at >= zalsa.last_changed_revision(durability)
    }

    unsafe fn maybe_changed_after(
        &self,
        zalsa: &Zalsa,
        index: DatabaseKeyIndex,
        input: Id,
        current_revision: Revision,
        entry: &Self::Entry,
    ) -> VerifyResult {
        // SAFETY: Guaranteed by the caller.
        let metadata = unsafe { &mut *entry.metadata.get() };
        if metadata.id.generation() > input.generation() {
            return VerifyResult::changed();
        }

        metadata.last_interned_at = current_revision;
        zalsa.event(&|| {
            Event::new(EventKind::DidValidateInternedValue {
                key: index,
                revision: current_revision,
            })
        });

        VerifyResult::unchanged()
    }
}

impl EvictionPolicy for ImmortalLru {
    const CAN_REUSE: bool = false;

    type Shard = LruShard;
    type Entry = LruEntry;
    type Durability = UnsafeCell<Durability>;

    fn new(revisions: NonZeroUsize) -> Self {
        debug_assert_eq!(revisions, IMMORTAL);
        Self {
            _lru: Lru {
                revision_queue: RevisionQueue::new(revisions),
            },
        }
    }

    fn new_entry(id: Id, last_interned_at: Revision) -> Self::Entry {
        Lru::new_entry(id, last_interned_at)
    }

    fn new_durability(durability: Durability) -> Self::Durability {
        Lru::new_durability(durability)
    }

    fn initial_metadata(
        zalsa_local: &ZalsaLocal,
        current_revision: Revision,
    ) -> (Durability, Revision) {
        Lru::initial_metadata(zalsa_local, current_revision)
    }

    unsafe fn id(entry: &Self::Entry) -> Id {
        // SAFETY: Guaranteed by the caller.
        unsafe { Lru::id(entry) }
    }

    unsafe fn serialized_metadata(
        entry: &Self::Entry,
        durability: &Self::Durability,
    ) -> (Durability, Revision) {
        // SAFETY: Guaranteed by the caller.
        unsafe { Lru::serialized_metadata(entry, durability) }
    }

    fn record_revision(&self, _revision: Revision) {}

    unsafe fn intern_existing(&self, existing: InternExisting<'_, Self>) -> Id {
        let InternExisting {
            zalsa,
            zalsa_local,
            index,
            current_revision,
            entry,
            durability,
            shard: _,
        } = existing;

        // SAFETY: Guaranteed by the caller.
        let (metadata, durability) =
            unsafe { (&mut *(*entry).metadata.get(), &mut *durability.get()) };

        if metadata.last_interned_at < current_revision {
            metadata.last_interned_at = current_revision;
            zalsa.event(&|| {
                Event::new(EventKind::DidValidateInternedValue {
                    key: index,
                    revision: current_revision,
                })
            });
        }

        if let Some((_, stamp)) = zalsa_local.active_query() {
            *durability = std::cmp::max(*durability, stamp.durability);
        }

        zalsa_local.report_tracked_read_revision(current_revision);
        metadata.id
    }

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

    unsafe fn insert_entry(
        &self,
        _shard: &mut Self::Shard,
        _entry: *const Self::Entry,
        _durability: &Self::Durability,
    ) {
    }

    unsafe fn report_tracked_read(
        &self,
        zalsa_local: &ZalsaLocal,
        _index: DatabaseKeyIndex,
        current_revision: Revision,
        _durability: &Self::Durability,
    ) {
        zalsa_local.report_tracked_read_revision(current_revision);
    }

    unsafe fn is_valid(
        &self,
        _zalsa: &Zalsa,
        _entry: &Self::Entry,
        _durability: &Self::Durability,
    ) -> bool {
        true
    }

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
}

impl Lru {
    fn is_reusable(&self, durability: Durability) -> bool {
        // Collecting higher durability values requires invalidating the revision for their
        // durability to avoid short-circuiting `maybe_changed_after`.
        durability == Durability::LOW
    }

    fn report_tracked_read_value(
        &self,
        zalsa_local: &ZalsaLocal,
        index: DatabaseKeyIndex,
        current_revision: Revision,
        durability: Durability,
    ) {
        if self.is_reusable(durability) {
            zalsa_local.report_tracked_read_simple(index, durability, current_revision);
        } else {
            // Reusing an interned ID can change a query without a corresponding input changing.
            zalsa_local.report_tracked_read_revision(current_revision);
        }
    }

    /// Finds the least-recently-used stale slot whose generation can be incremented.
    ///
    /// # Safety
    ///
    /// The caller must hold the shard lock, and all LRU entries must belong to live values in that
    /// shard.
    unsafe fn find_reusable_slot(
        &self,
        current_revision: Revision,
        shard: &mut LruShard,
    ) -> Option<ReusableSlot> {
        let mut cursor = shard.lru.back_mut();

        while let Some(entry) = cursor.as_cursor().clone_pointer() {
            let entry = UnsafeRef::into_raw(entry);

            // SAFETY: Guaranteed by the caller.
            let metadata = unsafe { &mut *(*entry).metadata.get() };

            if !self.revision_queue.is_stale(metadata.last_interned_at) {
                return None;
            }

            debug_assert!(metadata.last_interned_at < current_revision);

            let old_id = metadata.id;
            if let Some(new_id) = old_id.next_generation() {
                return Some(ReusableSlot {
                    entry,
                    old_id,
                    new_id,
                });
            }

            cursor.remove().unwrap();
            cursor = shard.lru.back_mut();
        }

        None
    }
}

/// Recovers a value from its intrusive LRU entry.
///
/// # Safety
///
/// `entry` must have been produced by `eviction::entry_ptr` for the same configuration, and its
/// value must remain live for the returned lifetime.
#[inline]
unsafe fn value_from_entry_ptr<'a, C: Configuration<Eviction = Lru>>(
    entry: *const LruEntry,
) -> &'a Value<C> {
    // SAFETY: `entry_ptr` derives `entry` from the enclosing value pointer.
    unsafe {
        &*entry
            .cast::<u8>()
            .sub(std::mem::offset_of!(Value<C>, eviction))
            .cast::<Value<C>>()
    }
}

/// Keep track of revisions in which interned values were read, to determine staleness.
///
/// An interned value is considered stale if it has not been read in the past `REVS`
/// revisions. However, we only consider revisions in which interned values were actually
/// read, as revisions may be created in bursts.
struct RevisionQueue {
    lock: Mutex<()>,
    /// Recent revisions, stored inline for the default configuration.
    revisions: SmallVec<[AtomicRevision; DEFAULT_REVISIONS]>,
}

// `#[salsa::interned(revisions = usize::MAX)]` disables garbage collection.
const IMMORTAL: NonZeroUsize = NonZeroUsize::MAX;

impl RevisionQueue {
    fn new(capacity: NonZeroUsize) -> Self {
        let revisions = if capacity == IMMORTAL {
            SmallVec::new()
        } else {
            (0..capacity.get())
                .map(|_| AtomicRevision::start())
                .collect()
        };

        Self {
            lock: Mutex::new(()),
            revisions,
        }
    }

    /// Record the given revision as active.
    #[inline]
    fn record(&self, revision: Revision) {
        debug_assert!(
            !self.revisions.is_empty(),
            "cannot record revisions when interned garbage collection is disabled"
        );

        // Fast-path: We already recorded this revision.
        if self.revisions[0].load() >= revision {
            return;
        }

        self.record_cold(revision);
    }

    #[cold]
    fn record_cold(&self, revision: Revision) {
        let _lock = self.lock.lock();

        // Another thread may have recorded this revision while we waited for the lock.
        if self.revisions[0].load() >= revision {
            return;
        }

        // Otherwise, update the queue, maintaining sorted order.
        //
        // Note that this should only happen once per revision.
        for i in (1..self.revisions.len()).rev() {
            self.revisions[i].store(self.revisions[i - 1].load());
        }

        self.revisions[0].store(revision);
    }

    /// Returns `true` if the given revision is old enough to be considered stale.
    #[inline]
    fn is_stale(&self, revision: Revision) -> bool {
        let Some(oldest) = self.revisions.last() else {
            return false;
        };
        let oldest = oldest.load();

        // If we have not recorded `REVS` revisions yet, nothing can be stale.
        if oldest == Revision::start() {
            return false;
        }

        revision < oldest
    }

    /// Returns `true` if the configured number of revisions have been recorded as active,
    /// i.e. enough data has been recorded to start garbage collection.
    #[inline]
    fn is_primed(&self) -> bool {
        self.revisions
            .last()
            .is_some_and(|oldest| oldest.load() > Revision::start())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::RevisionQueue;
    use crate::Revision;

    #[test]
    fn revision_queue_records_each_revision_once() {
        let queue = RevisionQueue::new(NonZeroUsize::new(2).unwrap());
        let revision = Revision::start().next();

        // Simulate two threads that both passed the fast-path check before taking the lock.
        queue.record_cold(revision);
        queue.record_cold(revision);

        assert_eq!(queue.revisions[0].load(), revision);
        assert_eq!(queue.revisions[1].load(), Revision::start());
    }
}
