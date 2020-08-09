use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::lru::Lru;
use crate::plumbing::DerivedQueryStorageOps;
use crate::plumbing::LruQueryStorageOps;
use crate::plumbing::QueryFunction;
use crate::plumbing::QueryStorageMassOps;
#[cfg(feature = "async")]
use crate::plumbing::{AsyncQueryFunction, QueryStorageOpsAsync};
use crate::plumbing::{QueryFunctionBase, QueryStorageOps, QueryStorageOpsSync};
use crate::runtime::{FxIndexMap, StampedValue};
use crate::{
    blocking_future::{BlockingFuture, BlockingFutureTrait},
    CycleError, Database, DatabaseKeyIndex, QueryBase, QueryDb, Revision, Runtime, SweepStrategy,
};
use parking_lot::RwLock;
use std::convert::TryFrom;
use std::marker::PhantomData;
use std::sync::Arc;

mod slot;
use slot::Slot;

pub use slot::WaitResult;

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
    Q: QueryFunctionBase,
    MP: MemoizationPolicy<Q>,
{
    group_index: u16,
    lru_list: Lru<Slot<Q, MP>>,
    slot_map: RwLock<FxIndexMap<Q::Key, Arc<Slot<Q, MP>>>>,
    policy: PhantomData<MP>,
}

impl<Q, MP> std::panic::RefUnwindSafe for DerivedStorage<Q, MP>
where
    Q: QueryFunctionBase,
    MP: MemoizationPolicy<Q>,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: std::panic::RefUnwindSafe,
{
}

pub trait MemoizationPolicy<Q>: Send + Sync
where
    Q: QueryBase,
{
    fn should_memoize_value(key: &Q::Key) -> bool;

    fn memoized_value_eq(old_value: &Q::Value, new_value: &Q::Value) -> bool;
}

pub enum AlwaysMemoizeValue {}
impl<Q> MemoizationPolicy<Q> for AlwaysMemoizeValue
where
    Q: QueryFunctionBase,
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
    Q: QueryFunctionBase,
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
    for<'f, 'd> Q: QueryFunction<'f, 'd>,
    MP: MemoizationPolicy<Q>,
{
    fn record_fetch(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        slot: &Arc<Slot<Q, MP>>,
        durability: Durability,
        changed_at: Revision,
    ) {
        if let Some(evicted) = self.lru_list.record_use(slot) {
            evicted.evict();
        }

        db.salsa_runtime()
            .report_query_read(slot.database_key_index(), durability, changed_at);
    }

    fn maybe_changed_since_get_slot(&self, input: &DatabaseKeyIndex) -> Arc<Slot<Q, MP>> {
        assert_eq!(input.group_index, self.group_index);
        assert_eq!(input.query_index, Q::QUERY_INDEX);
        self.slot_map
            .read()
            .get_index(input.key_index as usize)
            .unwrap()
            .1
            .clone()
    }

    fn slot(&self, key: &Q::Key) -> Arc<Slot<Q, MP>> {
        if let Some(v) = self.slot_map.read().get(key) {
            return v.clone();
        }

        let mut write = self.slot_map.write();
        let entry = write.entry(key.clone());
        let key_index = u32::try_from(entry.index()).unwrap();
        let database_key_index = DatabaseKeyIndex {
            group_index: self.group_index,
            query_index: Q::QUERY_INDEX,
            key_index: key_index,
        };
        entry
            .or_insert_with(|| Arc::new(Slot::new(key.clone(), database_key_index)))
            .clone()
    }
}

impl<Q, MP> QueryStorageOps<Q> for DerivedStorage<Q, MP>
where
    for<'f, 'd> Q: QueryFunction<'f, 'd>,
    MP: MemoizationPolicy<Q>,
{
    fn new(group_index: u16) -> Self {
        DerivedStorage {
            group_index,
            slot_map: RwLock::new(FxIndexMap::default()),
            lru_list: Default::default(),
            policy: PhantomData,
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
        let slot_map = self.slot_map.read();
        let key = slot_map.get_index(index.key_index as usize).unwrap().0;
        write!(fmt, "{}({:?})", Q::QUERY_NAME, key)
    }

    fn durability(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Durability {
        self.slot(key).durability(db)
    }

    fn entries<C>(&self, _db: &<Q as QueryDb<'_>>::DynDb) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        let slot_map = self.slot_map.read();
        slot_map
            .values()
            .filter_map(|slot| slot.as_table_entry())
            .collect()
    }

    fn peek(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Option<Q::Value> {
        self.slot(key).peek(db).map(|v| v.value)
    }
}

impl<Q, MP> QueryStorageOpsSync<Q> for DerivedStorage<Q, MP>
where
    for<'f, 'd> Q: QueryFunction<'f, 'd>,
    Q: QueryFunctionBase<
        BlockingFuture = BlockingFuture<WaitResult<<Q as QueryBase>::Value, DatabaseKeyIndex>>,
    >,
    MP: MemoizationPolicy<Q>,
{
    fn maybe_changed_since(
        &self,
        db: &mut <Q as QueryDb<'_>>::Db,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> bool {
        let slot = self.maybe_changed_since_get_slot(&input);
        crate::plumbing::sync_future(slot.maybe_changed_since(db, revision))
    }

    fn try_fetch(
        &self,
        db: &mut <Q as QueryDb<'_>>::Db,
        key: &Q::Key,
    ) -> Result<Q::Value, CycleError<DatabaseKeyIndex>> {
        let slot = self.slot(key);
        let StampedValue {
            value,
            durability,
            changed_at,
        } = crate::plumbing::sync_future(slot.read(db))?;

        self.record_fetch(db, &slot, durability, changed_at);

        Ok(value)
    }
}

#[cfg(feature = "async")]
impl<Q, MP> QueryStorageOpsAsync<Q> for DerivedStorage<Q, MP>
where
    for<'f, 'd> Q: AsyncQueryFunction<'f, 'd>,
    Q::BlockingFuture: Send,
    Q::Key: Send + Sync,
    Q::Value: Send + Sync,
    <Q::BlockingFuture as BlockingFutureTrait<WaitResult<Q::Value, DatabaseKeyIndex>>>::Promise:
        Send + Sync,
    MP: MemoizationPolicy<Q>,
{
    fn maybe_changed_since_async<'f>(
        &'f self,
        db: &'f mut <Q as AsyncQueryFunction<'_, '_>>::SendDb,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> crate::BoxFuture<'f, bool> {
        Box::pin(async move {
            let slot = self.maybe_changed_since_get_slot(&input);
            slot.maybe_changed_since(db, revision).await
        })
    }

    fn try_fetch_async<'f>(
        &'f self,
        db: &'f mut <Q as AsyncQueryFunction<'_, '_>>::SendDb,
        key: &'f Q::Key,
    ) -> crate::BoxFuture<'f, Result<Q::Value, CycleError<DatabaseKeyIndex>>> {
        Box::pin(async move {
            let slot = self.slot(key);
            let StampedValue {
                value,
                durability,
                changed_at,
            } = slot.read(db).await?;

            self.record_fetch(db, &slot, durability, changed_at);

            Ok(value)
        })
    }
}

impl<Q, MP> QueryStorageMassOps for DerivedStorage<Q, MP>
where
    for<'f, 'd> Q: QueryFunction<'f, 'd>,
    MP: MemoizationPolicy<Q>,
{
    fn sweep(&self, runtime: &Runtime, strategy: SweepStrategy) {
        let map_read = self.slot_map.read();
        let revision_now = runtime.current_revision();
        for slot in map_read.values() {
            slot.sweep(revision_now, strategy);
        }
    }
    fn purge(&self) {
        self.lru_list.purge();
        *self.slot_map.write() = Default::default();
    }
}

impl<Q, MP> LruQueryStorageOps for DerivedStorage<Q, MP>
where
    for<'f, 'd> Q: QueryFunction<'f, 'd>,
    MP: MemoizationPolicy<Q>,
{
    fn set_lru_capacity(&self, new_capacity: usize) {
        self.lru_list.set_lru_capacity(new_capacity);
    }
}

impl<Q, MP> DerivedQueryStorageOps<Q> for DerivedStorage<Q, MP>
where
    for<'f, 'd> Q: QueryFunction<'f, 'd>,
    MP: MemoizationPolicy<Q>,
{
    fn invalidate(&self, db: &mut <Q as QueryDb<'_>>::DynDb, key: &Q::Key) {
        db.salsa_runtime_mut()
            .with_incremented_revision(&mut |_new_revision| {
                let map_read = self.slot_map.read();

                if let Some(slot) = map_read.get(key) {
                    if let Some(durability) = slot.invalidate() {
                        return Some(durability);
                    }
                }

                None
            })
    }
}
