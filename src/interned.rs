use crate::debug::TableEntry;
use crate::intern_id::InternId;
use crate::plumbing::CycleDetected;
use crate::plumbing::HasQueryGroup;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::runtime::ChangedAt;
use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::Query;
use crate::{Database, DiscardIf, SweepStrategy};
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::collections::hash_map::Entry;
use std::convert::From;
use std::hash::Hash;

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
    Present {
        value: K,

        /// When was this intern'd?
        ///
        /// (This informs the "changed-at" result)
        interned_at: Revision,

        /// When was it accessed?
        ///
        /// (This informs the garbage collector)
        accessed_at: Revision,
    },

    /// Free-list -- the index is the next
    Free { next: Option<InternId> },
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
    fn intern_index(&self, db: &DB, key: &Q::Key) -> StampedValue<InternId> {
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
                // for the write lock.
                let index = *entry.get();
                match &tables.values[index.as_usize()] {
                    InternValue::Present {
                        value,
                        interned_at,
                        accessed_at,
                    } => {
                        debug_assert_eq!(owned_key2, *value);
                        debug_assert_eq!(*accessed_at, revision_now);
                        return StampedValue {
                            value: index,
                            changed_at: ChangedAt {
                                is_constant: false,
                                revision: *interned_at,
                            },
                        };
                    }

                    InternValue::Free { .. } => {
                        panic!("key {:?} should be present but is not", key,);
                    }
                }
            }
        };

        let index = match tables.first_free {
            None => {
                let index = InternId::from(tables.values.len());
                tables.values.push(InternValue::Present {
                    value: owned_key2,
                    interned_at: revision_now,
                    accessed_at: revision_now,
                });
                index
            }

            Some(i) => {
                let next_free = match &tables.values[i.as_usize()] {
                    InternValue::Free { next } => *next,
                    InternValue::Present { value, .. } => {
                        panic!(
                            "index {:?} was supposed to be free but contains {:?}",
                            i, value
                        );
                    }
                };

                tables.values[i.as_usize()] = InternValue::Present {
                    value: owned_key2,
                    interned_at: revision_now,
                    accessed_at: revision_now,
                };
                tables.first_free = next_free;
                i
            }
        };

        entry.insert(index);

        StampedValue {
            value: index,
            changed_at: ChangedAt {
                is_constant: false,
                revision: revision_now,
            },
        }
    }

    fn intern_check(&self, db: &DB, key: &Q::Key) -> Option<StampedValue<InternId>> {
        let revision_now = db.salsa_runtime().current_revision();

        // First, try with read lock -- this only works if `accessed_at` is up to date.
        {
            let tables = self.tables.read();
            let &index = tables.map.get(key)?;
            match &tables.values[index.as_usize()] {
                InternValue::Present {
                    interned_at,
                    accessed_at,
                    ..
                } => {
                    if *accessed_at == revision_now {
                        return Some(StampedValue {
                            value: index,
                            changed_at: ChangedAt {
                                is_constant: false,
                                revision: *interned_at,
                            },
                        });
                    }
                }

                InternValue::Free { .. } => {
                    panic!(
                        "key {:?} maps to index {:?} is free but should not be",
                        key, index
                    );
                }
            }
        }

        // Acquire write lock if necessary.
        let mut tables = self.tables.write();
        let &index = tables.map.get(key)?;
        match &mut tables.values[index.as_usize()] {
            InternValue::Present {
                interned_at,
                accessed_at,
                ..
            } => {
                *accessed_at = revision_now;

                return Some(StampedValue {
                    value: index,
                    changed_at: ChangedAt {
                        is_constant: false,
                        revision: *interned_at,
                    },
                });
            }

            InternValue::Free { .. } => {
                panic!(
                    "key {:?} maps to index {:?} is free but should not be",
                    key, index
                );
            }
        }
    }

    /// Given an index, lookup and clone its value, updating the
    /// `accessed_at` time if necessary.
    fn lookup_value<R>(
        &self,
        db: &DB,
        index: InternId,
        op: impl FnOnce(&Q::Key) -> R,
    ) -> StampedValue<R> {
        let index = index.as_usize();
        let revision_now = db.salsa_runtime().current_revision();

        {
            let tables = self.tables.read();
            debug_assert!(
                index < tables.values.len(),
                "interned key ``{:?}({})` is out of bounds",
                Q::default(),
                index,
            );
            match &tables.values[index] {
                InternValue::Present {
                    accessed_at,
                    interned_at,
                    value,
                } => {
                    if *accessed_at == revision_now {
                        return StampedValue {
                            value: op(value),
                            changed_at: ChangedAt {
                                is_constant: false,
                                revision: *interned_at,
                            },
                        };
                    }
                }

                InternValue::Free { .. } => panic!(
                    "interned key `{:?}({})` has been garbage collected",
                    Q::default(),
                    index,
                ),
            }
        }

        let mut tables = self.tables.write();
        match &mut tables.values[index] {
            InternValue::Present {
                accessed_at,
                interned_at,
                value,
            } => {
                *accessed_at = revision_now;

                return StampedValue {
                    value: op(value),
                    changed_at: ChangedAt {
                        is_constant: false,
                        revision: *interned_at,
                    },
                };
            }

            InternValue::Free { .. } => panic!(
                "interned key `{:?}({})` has been garbage collected",
                Q::default(),
                index,
            ),
        }
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
        let StampedValue { value, changed_at } = self.intern_index(db, key);

        db.salsa_runtime()
            .report_query_read(database_key, changed_at);

        Ok(<Q::Value>::from_intern_id(value))
    }

    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        _database_key: &DB::DatabaseKey,
    ) -> bool {
        match self.intern_check(db, key) {
            Some(StampedValue {
                value: _,
                changed_at,
            }) => changed_at.changed_since(revision),
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
            let discard = match strategy.discard_if {
                DiscardIf::Never => false,

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
                DiscardIf::Always | DiscardIf::Outdated => match values[intern_index.as_usize()] {
                    InternValue::Present { accessed_at, .. } => accessed_at < revision_now,

                    InternValue::Free { .. } => {
                        panic!(
                            "key {:?} maps to index {:?} which is free",
                            key, intern_index
                        );
                    }
                },
            };

            if discard {
                values[intern_index.as_usize()] = InternValue::Free { next: *first_free };
                *first_free = Some(*intern_index);
            }

            !discard
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
        let StampedValue { value, changed_at } =
            interned_storage.lookup_value(db, index, Clone::clone);

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
        let StampedValue {
            value: (),
            changed_at,
        } = interned_storage.lookup_value(db, index, |_| ());

        changed_at.changed_since(revision)
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
