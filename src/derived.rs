use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::hash::FxDashMap;
use crate::lru::Lru;
use crate::plumbing::DerivedQueryStorageOps;
use crate::plumbing::LruQueryStorageOps;
use crate::plumbing::QueryFunction;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::runtime::StampedValue;
use crate::Runtime;
use crate::{Database, DatabaseKeyIndex, QueryDb, Revision};
use crossbeam_utils::atomic::AtomicCell;
use std::borrow::Borrow;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;

mod slot;
use slot::Slot;

/// Memoized queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub type MemoizedStorage<Q> = DerivedStorage<Q, AlwaysMemoizeValue>;

/// "Dependency" queries just track their dependencies and not the
/// actual value (which they produce on demand). This lessens the
/// storage requirements.
pub type DependencyStorage<Q> = DerivedStorage<Q, NeverMemoizeValue>;

/// Handles storage where the value is 'derived' by executing a
/// function (in contrast to "inputs").
pub struct DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    group_index: u16,
    lru_list: Lru<Slot<Q, MP>>,
    indices: AtomicCell<u32>,
    index_map: FxDashMap<Q::Key, DerivedKeyIndex>,
    slot_map: FxDashMap<DerivedKeyIndex, KeySlot<Q, MP>>,
    policy: PhantomData<MP>,
}

struct KeySlot<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    key: Q::Key,
    slot: Arc<Slot<Q, MP>>,
}

type DerivedKeyIndex = u32;

impl<Q, MP> std::panic::RefUnwindSafe for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: std::panic::RefUnwindSafe,
{
}

pub trait MemoizationPolicy<Q>: Send + Sync
where
    Q: QueryFunction,
{
    fn should_memoize_value(key: &Q::Key) -> bool;

    fn memoized_value_eq(old_value: &Q::Value, new_value: &Q::Value) -> bool;
}

pub enum AlwaysMemoizeValue {}
impl<Q> MemoizationPolicy<Q> for AlwaysMemoizeValue
where
    Q: QueryFunction,
    Q::Value: Eq,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        true
    }

    fn memoized_value_eq(old_value: &Q::Value, new_value: &Q::Value) -> bool {
        old_value == new_value
    }
}

pub enum NeverMemoizeValue {}
impl<Q> MemoizationPolicy<Q> for NeverMemoizeValue
where
    Q: QueryFunction,
{
    fn should_memoize_value(_key: &Q::Key) -> bool {
        false
    }

    fn memoized_value_eq(_old_value: &Q::Value, _new_value: &Q::Value) -> bool {
        panic!("cannot reach since we never memoize")
    }
}

impl<Q, MP> DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn slot_for_key(&self, key: &Q::Key) -> Arc<Slot<Q, MP>> {
        // Common case: get an existing key
        if let Some(v) = self.index_map.get(key) {
            let index = *v;

            // release the read-write lock early, for no particular reason
            // apart from it bothers me
            drop(v);

            return self.slot_for_key_index(index);
        }

        // Less common case: (potentially) create a new slot
        match self.index_map.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => self.slot_for_key_index(*entry.get()),
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let key_index = self.indices.fetch_add(1);
                let database_key_index = DatabaseKeyIndex {
                    group_index: self.group_index,
                    query_index: Q::QUERY_INDEX,
                    key_index,
                };
                let slot = Arc::new(Slot::new(key.clone(), database_key_index));
                // Subtle: store the new slot *before* the new index, so that
                // other threads only see the new index once the slot is also available.
                self.slot_map.insert(
                    key_index,
                    KeySlot {
                        key: key.clone(),
                        slot: slot.clone(),
                    },
                );
                entry.insert(key_index);
                slot
            }
        }
    }

    fn slot_for_key_index(&self, index: DerivedKeyIndex) -> Arc<Slot<Q, MP>> {
        return self.slot_map.get(&index).unwrap().slot.clone();
    }

    fn slot_for_db_index(&self, index: DatabaseKeyIndex) -> Arc<Slot<Q, MP>> {
        assert_eq!(index.group_index, self.group_index);
        assert_eq!(index.query_index, Q::QUERY_INDEX);
        self.slot_for_key_index(index.key_index)
    }
}

impl<Q, MP> QueryStorageOps<Q> for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    const CYCLE_STRATEGY: crate::plumbing::CycleRecoveryStrategy = Q::CYCLE_STRATEGY;

    fn new(group_index: u16) -> Self {
        DerivedStorage {
            group_index,
            index_map: Default::default(),
            slot_map: Default::default(),
            lru_list: Default::default(),
            policy: PhantomData,
            indices: Default::default(),
        }
    }

    fn fmt_index(
        &self,
        _db: &<Q as QueryDb<'_>>::DynDb,
        index: DatabaseKeyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        assert_eq!(index.group_index, self.group_index);
        assert_eq!(index.query_index, Q::QUERY_INDEX);
        let key_slot = self.slot_map.get(&index.key_index).unwrap();
        write!(fmt, "{}({:?})", Q::QUERY_NAME, key_slot.key)
    }

    fn maybe_changed_after(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> bool {
        debug_assert!(revision < db.salsa_runtime().current_revision());
        let slot = self.slot_for_db_index(input);
        slot.maybe_changed_after(db, revision)
    }

    fn fetch(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Q::Value {
        db.unwind_if_cancelled();

        let slot = self.slot_for_key(key);
        let StampedValue {
            value,
            durability,
            changed_at,
        } = slot.read(db);

        if let Some(evicted) = self.lru_list.record_use(&slot) {
            evicted.evict();
        }

        db.salsa_runtime()
            .report_query_read_and_unwind_if_cycle_resulted(
                slot.database_key_index(),
                durability,
                changed_at,
            );

        value
    }

    fn durability(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Durability {
        self.slot_for_key(key).durability(db)
    }

    fn entries<C>(&self, _db: &<Q as QueryDb<'_>>::DynDb) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        self.slot_map
            .iter()
            .filter_map(|r| r.value().slot.as_table_entry())
            .collect()
    }
}

impl<Q, MP> QueryStorageMassOps for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn purge(&self) {
        self.lru_list.purge();
        self.indices.store(0);
        self.index_map.clear();
        self.slot_map.clear();
    }
}

impl<Q, MP> LruQueryStorageOps for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn set_lru_capacity(&self, new_capacity: usize) {
        self.lru_list.set_lru_capacity(new_capacity);
    }
}

impl<Q, MP> DerivedQueryStorageOps<Q> for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn invalidate<S>(&self, runtime: &mut Runtime, key: &S)
    where
        S: Eq + Hash,
        Q::Key: Borrow<S>,
    {
        runtime.with_incremented_revision(|new_revision| {
            if let Some(key_index) = self.index_map.get(key) {
                let slot = self.slot_for_key_index(*key_index);
                if let Some(durability) = slot.invalidate(new_revision) {
                    return Some(durability);
                }
            }
            None
        })
    }
}
