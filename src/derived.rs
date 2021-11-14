use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::plumbing::DerivedQueryStorageOps;
use crate::plumbing::LruQueryStorageOps;
use crate::plumbing::QueryFunction;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::runtime::local_state::QueryInputs;
use crate::runtime::local_state::QueryRevisions;
use crate::Runtime;
use crate::{Database, DatabaseKeyIndex, QueryDb, Revision};
use std::borrow::Borrow;
use std::hash::Hash;
use std::marker::PhantomData;

mod execute;
mod fetch;
mod key_to_key_index;
mod lru;
mod maybe_changed_after;
mod memo;
mod sync;

//mod slot;
//use slot::Slot;

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
    lru: lru::Lru,
    key_map: key_to_key_index::KeyToKeyIndex<Q::Key>,
    memo_map: memo::MemoMap<Q::Value>,
    sync_map: sync::SyncMap,
    policy: PhantomData<MP>,
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
    fn database_key_index(&self, key_index: DerivedKeyIndex) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            group_index: self.group_index,
            query_index: Q::QUERY_INDEX,
            key_index: key_index,
        }
    }

    fn assert_our_key_index(&self, index: DatabaseKeyIndex) {
        assert_eq!(index.group_index, self.group_index);
        assert_eq!(index.query_index, Q::QUERY_INDEX);
    }

    fn key_index(&self, index: DatabaseKeyIndex) -> DerivedKeyIndex {
        self.assert_our_key_index(index);
        index.key_index
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
            lru: Default::default(),
            key_map: Default::default(),
            memo_map: Default::default(),
            sync_map: Default::default(),
            policy: PhantomData,
        }
    }

    fn fmt_index(
        &self,
        _db: &<Q as QueryDb<'_>>::DynDb,
        index: DatabaseKeyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let key_index = self.key_index(index);
        let key = self.key_map.key_for_key_index(key_index);
        write!(fmt, "{}({:?})", Q::QUERY_NAME, key)
    }

    fn maybe_changed_after(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        database_key_index: DatabaseKeyIndex,
        revision: Revision,
    ) -> bool {
        debug_assert!(revision < db.salsa_runtime().current_revision());
        let key_index = self.key_index(database_key_index);
        self.maybe_changed_after(db, key_index, revision)
    }

    fn fetch(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Q::Value {
        let key_index = self.key_map.key_index_for_key(key);
        self.fetch(db, key_index)
    }

    fn durability(&self, _db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Durability {
        let key_index = self.key_map.key_index_for_key(key);
        if let Some(memo) = self.memo_map.get(key_index) {
            memo.revisions.durability
        } else {
            Durability::LOW
        }
    }

    fn entries<C>(&self, _db: &<Q as QueryDb<'_>>::DynDb) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        self.memo_map
            .iter()
            .map(|(key_index, memo)| {
                let key = self.key_map.key_for_key_index(key_index);
                TableEntry::new(key, memo.value.clone())
            })
            .collect()
    }
}

impl<Q, MP> QueryStorageMassOps for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn purge(&self) {
        self.lru.set_capacity(0);
        self.memo_map.clear();
    }
}

impl<Q, MP> LruQueryStorageOps for DerivedStorage<Q, MP>
where
    Q: QueryFunction,
    MP: MemoizationPolicy<Q>,
{
    fn set_lru_capacity(&self, new_capacity: usize) {
        self.lru.set_capacity(new_capacity);
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
            let key_index = self.key_map.existing_key_index_for_key(key)?;
            let memo = self.memo_map.get(key_index)?;
            let invalidated_revisions = QueryRevisions {
                changed_at: new_revision,
                durability: memo.revisions.durability,
                inputs: QueryInputs::Untracked,
            };
            let new_memo = memo::Memo::new(
                memo.value.clone(),
                memo.verified_at.load(),
                invalidated_revisions,
            );
            self.memo_map.insert(key_index, new_memo);
            Some(memo.revisions.durability)
        })
    }
}
