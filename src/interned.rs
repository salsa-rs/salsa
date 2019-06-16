use crate::debug::TableEntry;
use crate::intern_id::InternId;
use crate::plumbing::CycleDetected;
use crate::plumbing::HasQueryGroup;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::runtime::ChangedAt;
use crate::runtime::Revision;
use crate::Query;
use crate::{Database, DiscardIf, SweepStrategy};
use crossbeam::atomic::AtomicCell;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::collections::hash_map::Entry;
use std::convert::From;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

/// Handles storage where the value is 'derived' by executing a
/// function (in contrast to "inputs").
pub struct InternedStorage<DB, Q>
where
    Q: Query<DB>,
    Q::Value: InternKey,
    DB: Database,
{
    tables: RwLock<InternTables<Q::Key>>,
}

/// Storage for the looking up interned things.
pub struct LookupInternedStorage<DB, Q, IQ>
where
    Q: Query<DB>,
    Q::Key: InternKey,
    Q::Value: Eq + Hash,
    IQ: Query<
        DB,
        Key = Q::Value,
        Value = Q::Key,
        Group = Q::Group,
        GroupStorage = Q::GroupStorage,
        GroupKey = Q::GroupKey,
    >,
    DB: Database,
{
    phantom: std::marker::PhantomData<(Q::Key, IQ)>,
}

struct InternTables<K> {
    /// Map from the key to the corresponding intern-index.
    map: FxHashMap<K, InternId>,

    /// For each valid intern-index, stores the interned value. When
    /// an interned value is GC'd, the entry is set to
    /// `InternValue::Free` with the next free item.
    values: Vec<InternValue<K>>,

    /// Index of the first free intern-index, if any.
    first_free: Option<InternId>,
}

/// Trait implemented for the "key" that results from a
/// `#[salsa::intern]` query.  This is basically meant to be a
/// "newtype"'d `u32`.
pub trait InternKey {
    /// Create an instance of the intern-key from a `u32` value.
    fn from_intern_id(v: InternId) -> Self;

    /// Extract the `u32` with which the intern-key was created.
    fn as_intern_id(&self) -> InternId;
}

impl InternKey for InternId {
    fn from_intern_id(v: InternId) -> InternId {
        v
    }

    fn as_intern_id(&self) -> InternId {
        *self
    }
}

enum InternValue<K> {
    /// The value has not been gc'd.
    Present { slot: Arc<Slot<K>> },

    /// Free-list -- the index is the next
    Free { next: Option<InternId> },
}

#[derive(Debug)]
struct Slot<K> {
    /// Index of this slot in the list of interned values;
    /// set to None if gc'd.
    index: InternId,

    /// Value that was interned.
    value: K,

    /// When was this intern'd?
    ///
    /// (This informs the "changed-at" result)
    interned_at: Revision,

    /// When was it accessed? Equal to `None` if this slot has
    /// been garbage collected.
    ///
    /// This has a subtle interaction with the garbage
    /// collector. First, we will never GC anything accessed in the
    /// current revision.
    ///
    /// To protect a slot from being GC'd, we can therefore update the
    /// `accessed_at` field to `Some(revision_now)` before releasing
    /// the read-lock on our interning tables.
    accessed_at: AtomicCell<Option<Revision>>,
}

impl<DB, Q> std::panic::RefUnwindSafe for InternedStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: InternKey,
    Q::Value: std::panic::RefUnwindSafe,
{
}

impl<DB, Q> Default for InternedStorage<DB, Q>
where
    Q: Query<DB>,
    Q::Key: Eq + Hash,
    Q::Value: InternKey,
    DB: Database,
{
    fn default() -> Self {
        InternedStorage {
            tables: RwLock::new(InternTables::default()),
        }
    }
}

impl<DB, Q, IQ> Default for LookupInternedStorage<DB, Q, IQ>
where
    Q: Query<DB>,
    Q::Key: InternKey,
    Q::Value: Eq + Hash,
    IQ: Query<
        DB,
        Key = Q::Value,
        Value = Q::Key,
        Group = Q::Group,
        GroupStorage = Q::GroupStorage,
        GroupKey = Q::GroupKey,
    >,
    DB: Database,
{
    fn default() -> Self {
        LookupInternedStorage {
            phantom: std::marker::PhantomData,
        }
    }
}

impl<K: Debug + Hash + Eq> InternTables<K> {
    /// Returns the slot for the given key.
    ///
    /// The slot will have its "accessed at" field updated to its current revision,
    /// ensuring that it cannot be GC'd until the current queries complete.
    fn slot_for_key(&self, key: &K, revision_now: Revision) -> Option<Arc<Slot<K>>> {
        let index = self.map.get(key)?;
        Some(self.slot_for_index(*index, revision_now))
    }

    /// Returns the slot at the given index.
    ///
    /// The slot will have its "accessed at" field updated to its current revision,
    /// ensuring that it cannot be GC'd until the current queries complete.
    fn slot_for_index(&self, index: InternId, revision_now: Revision) -> Arc<Slot<K>> {
        match &self.values[index.as_usize()] {
            InternValue::Present { slot } => {
                // Subtle: we must update the "accessed at" to the
                // current revision *while the lock is held* to
                // prevent this slot from being GC'd.
                let updated = slot.try_update_accessed_at(revision_now);
                assert!(
                    updated,
                    "failed to update slot {:?} while holding read lock",
                    slot
                );
                slot.clone()
            }
            InternValue::Free { .. } => {
                panic!("index {:?} is free but should not be", index);
            }
        }
    }
}

impl<K> Default for InternTables<K>
where
    K: Eq + Hash,
{
    fn default() -> Self {
        Self {
            map: Default::default(),
            values: Default::default(),
            first_free: Default::default(),
        }
    }
}

impl<DB, Q> InternedStorage<DB, Q>
where
    Q: Query<DB>,
    Q::Key: Eq + Hash + Clone,
    Q::Value: InternKey,
    DB: Database,
{
    /// If `key` has already been interned, returns its slot. Otherwise, creates a new slot.
    ///
    /// In either case, the `accessed_at` field of the slot is updated
    /// to the current revision, ensuring that the slot cannot be GC'd
    /// while the current queries execute.
    fn intern_index(&self, db: &DB, key: &Q::Key) -> Arc<Slot<Q::Key>> {
        if let Some(i) = self.intern_check(db, key) {
            return i;
        }

        let owned_key1 = key.to_owned();
        let owned_key2 = owned_key1.clone();
        let revision_now = db.salsa_runtime().current_revision();

        let mut tables = self.tables.write();
        let tables = &mut *tables;
        let entry = match tables.map.entry(owned_key1) {
            Entry::Vacant(entry) => entry,
            Entry::Occupied(entry) => {
                // Somebody inserted this key while we were waiting
                // for the write lock. In this case, we don't need to
                // update the `accessed_at` field because they should
                // have already done so!
                let index = *entry.get();
                match &tables.values[index.as_usize()] {
                    InternValue::Present { slot } => {
                        debug_assert_eq!(owned_key2, slot.value);
                        debug_assert_eq!(slot.accessed_at.load(), Some(revision_now));
                        return slot.clone();
                    }

                    InternValue::Free { .. } => {
                        panic!("key {:?} should be present but is not", key,);
                    }
                }
            }
        };

        let create_slot = |index: InternId| {
            Arc::new(Slot {
                index,
                value: owned_key2,
                interned_at: revision_now,
                accessed_at: AtomicCell::new(Some(revision_now)),
            })
        };

        let (slot, index);
        match tables.first_free {
            None => {
                index = InternId::from(tables.values.len());
                slot = create_slot(index);
                tables
                    .values
                    .push(InternValue::Present { slot: slot.clone() });
            }

            Some(i) => {
                index = i;
                slot = create_slot(index);

                let next_free = match &tables.values[i.as_usize()] {
                    InternValue::Free { next } => *next,
                    InternValue::Present { slot } => {
                        panic!(
                            "index {:?} was supposed to be free but contains {:?}",
                            i, slot.value
                        );
                    }
                };

                tables.values[index.as_usize()] = InternValue::Present { slot: slot.clone() };
                tables.first_free = next_free;
            }
        }

        entry.insert(index);

        slot
    }

    fn intern_check(&self, db: &DB, key: &Q::Key) -> Option<Arc<Slot<Q::Key>>> {
        let revision_now = db.salsa_runtime().current_revision();
        let slot = self.tables.read().slot_for_key(key, revision_now)?;
        Some(slot)
    }

    /// Given an index, lookup and clone its value, updating the
    /// `accessed_at` time if necessary.
    fn lookup_value(&self, db: &DB, index: InternId) -> Arc<Slot<Q::Key>> {
        let revision_now = db.salsa_runtime().current_revision();
        self.tables.read().slot_for_index(index, revision_now)
    }
}

impl<DB, Q> QueryStorageOps<DB, Q> for InternedStorage<DB, Q>
where
    Q: Query<DB>,
    Q::Value: InternKey,
    DB: Database,
{
    fn try_fetch(
        &self,
        db: &DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
    ) -> Result<Q::Value, CycleDetected> {
        let slot = self.intern_index(db, key);
        let changed_at = slot.interned_at;
        let index = slot.index;
        db.salsa_runtime().report_query_read(
            database_key,
            ChangedAt {
                is_constant: false,
                revision: changed_at,
            },
        );
        Ok(<Q::Value>::from_intern_id(index))
    }

    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        _database_key: &DB::DatabaseKey,
    ) -> bool {
        match self.intern_check(db, key) {
            Some(slot) => slot.interned_at > revision,
            None => true,
        }
    }

    fn is_constant(&self, _db: &DB, _key: &Q::Key, _database_key: &DB::DatabaseKey) -> bool {
        false
    }

    fn entries<C>(&self, _db: &DB) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        let tables = self.tables.read();
        tables
            .map
            .iter()
            .map(|(key, index)| {
                TableEntry::new(key.clone(), Some(<Q::Value>::from_intern_id(*index)))
            })
            .collect()
    }
}

impl<DB, Q> QueryStorageMassOps<DB> for InternedStorage<DB, Q>
where
    Q: Query<DB>,
    Q::Value: InternKey,
    DB: Database,
{
    fn sweep(&self, db: &DB, strategy: SweepStrategy) {
        let mut tables = self.tables.write();
        let revision_now = db.salsa_runtime().current_revision();
        let InternTables {
            map,
            values,
            first_free,
        } = &mut *tables;
        map.retain(|key, intern_index| {
            match strategy.discard_if {
                DiscardIf::Never => true,

                // NB: Interned keys *never* discard keys unless they
                // are outdated, regardless of the sweep strategy. This is
                // because interned queries are not deterministic;
                // if we were to remove a value from the current revision,
                // and the query were later executed again, it would not necessarily
                // produce the same intern key the second time. This would wreak
                // havoc. See the test `discard_during_same_revision` for an example.
                //
                // Keys that have not (yet) been accessed during this
                // revision don't have this problem. Anything
                // dependent on them would regard itself as dirty if
                // they are removed and also be forced to re-execute.
                DiscardIf::Always | DiscardIf::Outdated => match &values[intern_index.as_usize()] {
                    InternValue::Present { slot, .. } => {
                        if slot.try_collect(revision_now) {
                            values[intern_index.as_usize()] =
                                InternValue::Free { next: *first_free };
                            *first_free = Some(*intern_index);
                            false
                        } else {
                            true
                        }
                    }

                    InternValue::Free { .. } => {
                        panic!(
                            "key {:?} maps to index {:?} which is free",
                            key, intern_index
                        );
                    }
                },
            }
        });
    }
}

impl<DB, Q, IQ> QueryStorageOps<DB, Q> for LookupInternedStorage<DB, Q, IQ>
where
    Q: Query<DB>,
    Q::Key: InternKey,
    Q::Value: Eq + Hash,
    IQ: Query<
        DB,
        Key = Q::Value,
        Value = Q::Key,
        Storage = InternedStorage<DB, IQ>,
        Group = Q::Group,
        GroupStorage = Q::GroupStorage,
        GroupKey = Q::GroupKey,
    >,
    DB: Database + HasQueryGroup<Q::Group>,
{
    fn try_fetch(
        &self,
        db: &DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
    ) -> Result<Q::Value, CycleDetected> {
        let index = key.as_intern_id();

        let group_storage = <DB as HasQueryGroup<Q::Group>>::group_storage(db);
        let interned_storage = IQ::query_storage(group_storage);
        let slot = interned_storage.lookup_value(db, index);
        let changed_at = ChangedAt {
            is_constant: false,
            revision: slot.interned_at,
        };
        let value = slot.value.clone();
        db.salsa_runtime()
            .report_query_read(database_key, changed_at);
        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        _database_key: &DB::DatabaseKey,
    ) -> bool {
        let index = key.as_intern_id();

        // NB. This will **panic** if `key` has been removed from the
        // map, whereas you might expect it to return true in that
        // event.  But I think this is ok. You have to ask yourself,
        // where did this (invalid) key K come from? There are two
        // options:
        //
        // ## Some query Q1 obtained the key K by interning a value:
        //
        // In that case, Q1 has a prior input that computes K. This
        // input must be invalid and hence Q1 must be considered to
        // have changed, so it shouldn't be asking if we have changed.
        //
        // ## Some query Q1 was given K as an input:
        //
        // In that case, the query Q1 must be invoked (ultimately) by
        // some query Q2 that computed K. This implies that K must be
        // the result of *some* valid interning call, and therefore
        // that it should be a valid key now (and not pointing at a
        // free slot or out of bounds).

        let group_storage = <DB as HasQueryGroup<Q::Group>>::group_storage(db);
        let interned_storage = IQ::query_storage(group_storage);
        let slot = interned_storage.lookup_value(db, index);
        slot.interned_at > revision
    }

    fn is_constant(&self, _db: &DB, _key: &Q::Key, _database_key: &DB::DatabaseKey) -> bool {
        false
    }

    fn entries<C>(&self, db: &DB) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        let group_storage = <DB as HasQueryGroup<Q::Group>>::group_storage(db);
        let interned_storage = IQ::query_storage(group_storage);
        let tables = interned_storage.tables.read();
        tables
            .map
            .iter()
            .map(|(key, index)| {
                TableEntry::new(<Q::Key>::from_intern_id(*index), Some(key.clone()))
            })
            .collect()
    }
}

impl<DB, Q, IQ> QueryStorageMassOps<DB> for LookupInternedStorage<DB, Q, IQ>
where
    Q: Query<DB>,
    Q::Key: InternKey,
    Q::Value: Eq + Hash,
    IQ: Query<
        DB,
        Key = Q::Value,
        Value = Q::Key,
        Group = Q::Group,
        GroupStorage = Q::GroupStorage,
        GroupKey = Q::GroupKey,
    >,
    DB: Database,
{
    fn sweep(&self, _db: &DB, _strategy: SweepStrategy) {}
}

impl<K> Slot<K> {
    /// Updates the `accessed_at` time to be `revision_now` (if
    /// necessary).  Returns true if the update was successful, or
    /// false if the slot has been GC'd in the interim.
    fn try_update_accessed_at(&self, revision_now: Revision) -> bool {
        if let Some(accessed_at) = self.accessed_at.load() {
            match self
                .accessed_at
                .compare_exchange(Some(accessed_at), Some(revision_now))
            {
                Ok(_) => true,
                Err(Some(r)) => {
                    // Somebody was racing with us to update the field -- but they
                    // also updated it to revision now, so that's cool.
                    debug_assert_eq!(r, revision_now);
                    true
                }
                Err(None) => {
                    // The garbage collector was racing with us and it swept this
                    // slot before we could mark it as accessed.
                    false
                }
            }
        } else {
            false
        }
    }

    /// Invoked during sweeping to try and collect this slot. Fails if
    /// the slot has been accessed in the current revision. Note that
    /// this access could be racing with the attempt to collect (in
    /// particular, when verifying dependencies).
    fn try_collect(&self, revision_now: Revision) -> bool {
        let accessed_at = self.accessed_at.load().unwrap();
        if accessed_at < revision_now {
            match self.accessed_at.compare_exchange(Some(accessed_at), None) {
                Ok(_) => true,
                Err(r) => {
                    // The only one racing with us can be a
                    // verification attempt, which will always bump
                    // `accessed_at` to the current revision.
                    debug_assert_eq!(r, Some(revision_now));
                    false
                }
            }
        } else {
            false
        }
    }
}
