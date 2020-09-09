use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::revision::Revision;
use crate::runtime::{FxIndexMap, StampedValue};
use crate::CycleError;
use crate::Database;
use crate::Query;
use crate::{DatabaseKeyIndex, QueryDb, Runtime, SweepStrategy};
use indexmap::map::Entry;
use log::debug;
use parking_lot::RwLock;
use std::convert::TryFrom;
use std::sync::Arc;

/// Input queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub struct InputStorage<Q>
where
    Q: Query,
{
    group_index: u16,
    slots: RwLock<FxIndexMap<Q::Key, Arc<Slot<Q>>>>,
}

struct Slot<Q>
where
    Q: Query,
{
    key: Q::Key,
    database_key_index: DatabaseKeyIndex,
    stamped_value: RwLock<StampedValue<Q::Value>>,
}

impl<Q> std::panic::RefUnwindSafe for InputStorage<Q>
where
    Q: Query,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: std::panic::RefUnwindSafe,
{
}

impl<Q> InputStorage<Q>
where
    Q: Query,
{
    fn slot(&self, key: &Q::Key) -> Option<Arc<Slot<Q>>> {
        self.slots.read().get(key).cloned()
    }
}

impl<Q> QueryStorageOps<Q> for InputStorage<Q>
where
    Q: Query,
{
    fn new(group_index: u16) -> Self {
        InputStorage {
            group_index,
            slots: Default::default(),
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
        let slot_map = self.slots.read();
        let key = slot_map.get_index(index.key_index as usize).unwrap().0;
        write!(fmt, "{}({:?})", Q::QUERY_NAME, key)
    }

    fn maybe_changed_since(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> bool {
        assert_eq!(input.group_index, self.group_index);
        assert_eq!(input.query_index, Q::QUERY_INDEX);
        let slot = self
            .slots
            .read()
            .get_index(input.key_index as usize)
            .unwrap()
            .1
            .clone();
        slot.maybe_changed_since(db, revision)
    }

    fn try_fetch(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        key: &Q::Key,
    ) -> Result<Q::Value, CycleError<DatabaseKeyIndex>> {
        let slot = self
            .slot(key)
            .unwrap_or_else(|| panic!("no value set for {:?}({:?})", Q::default(), key));

        let StampedValue {
            value,
            durability,
            changed_at,
        } = slot.stamped_value.read().clone();

        db.salsa_runtime()
            .report_query_read(slot.database_key_index, durability, changed_at);

        Ok(value)
    }

    fn durability(&self, _db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Durability {
        match self.slot(key) {
            Some(slot) => slot.stamped_value.read().durability,
            None => panic!("no value set for {:?}({:?})", Q::default(), key),
        }
    }

    fn entries<C>(&self, _db: &<Q as QueryDb<'_>>::DynDb) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>,
    {
        let slots = self.slots.read();
        slots
            .values()
            .map(|slot| {
                TableEntry::new(
                    slot.key.clone(),
                    Some(slot.stamped_value.read().value.clone()),
                )
            })
            .collect()
    }
}

impl<Q> Slot<Q>
where
    Q: Query,
{
    fn maybe_changed_since(&self, _db: &<Q as QueryDb<'_>>::DynDb, revision: Revision) -> bool {
        debug!(
            "maybe_changed_since(slot={:?}, revision={:?})",
            self, revision,
        );

        let changed_at = self.stamped_value.read().changed_at;

        debug!("maybe_changed_since: changed_at = {:?}", changed_at);

        changed_at > revision
    }
}

impl<Q> QueryStorageMassOps for InputStorage<Q>
where
    Q: Query,
{
    fn sweep(&self, _runtime: &Runtime, _strategy: SweepStrategy) {}
    fn purge(&self) {
        *self.slots.write() = Default::default();
    }
}

impl<Q> InputQueryStorageOps<Q> for InputStorage<Q>
where
    Q: Query,
{
    fn set(
        &self,
        db: &mut <Q as QueryDb<'_>>::DynDb,
        key: &Q::Key,
        value: Q::Value,
        durability: Durability,
    ) {
        log::debug!(
            "{:?}({:?}) = {:?} ({:?})",
            Q::default(),
            key,
            value,
            durability
        );

        // The value is changing, so we need a new revision (*). We also
        // need to update the 'last changed' revision by invoking
        // `guard.mark_durability_as_changed`.
        //
        // CAREFUL: This will block until the global revision lock can
        // be acquired. If there are still queries executing, they may
        // need to read from this input. Therefore, we wait to acquire
        // the lock on `map` until we also hold the global query write
        // lock.
        //
        // (*) Technically, since you can't presently access an input
        // for a non-existent key, and you can't enumerate the set of
        // keys, we only need a new revision if the key used to
        // exist. But we may add such methods in the future and this
        // case doesn't generally seem worth optimizing for.
        let mut value = Some(value);
        db.salsa_runtime_mut()
            .with_incremented_revision(&mut |next_revision| {
                let mut slots = self.slots.write();

                // Do this *after* we acquire the lock, so that we are not
                // racing with somebody else to modify this same cell.
                // (Otherwise, someone else might write a *newer* revision
                // into the same cell while we block on the lock.)
                let stamped_value = StampedValue {
                    value: value.take().unwrap(),
                    durability,
                    changed_at: next_revision,
                };

                match slots.entry(key.clone()) {
                    Entry::Occupied(entry) => {
                        let mut slot_stamped_value = entry.get().stamped_value.write();
                        let old_durability = slot_stamped_value.durability;
                        *slot_stamped_value = stamped_value;
                        Some(old_durability)
                    }

                    Entry::Vacant(entry) => {
                        let key_index = u32::try_from(entry.index()).unwrap();
                        let database_key_index = DatabaseKeyIndex {
                            group_index: self.group_index,
                            query_index: Q::QUERY_INDEX,
                            key_index,
                        };
                        entry.insert(Arc::new(Slot {
                            key: key.clone(),
                            database_key_index,
                            stamped_value: RwLock::new(stamped_value),
                        }));
                        None
                    }
                }
            });
    }
}

/// Check that `Slot<Q, MP>: Send + Sync` as long as
/// `DB::DatabaseData: Send + Sync`, which in turn implies that
/// `Q::Key: Send + Sync`, `Q::Value: Send + Sync`.
#[allow(dead_code)]
fn check_send_sync<Q>()
where
    Q: Query,
    Q::Key: Send + Sync,
    Q::Value: Send + Sync,
{
    fn is_send_sync<T: Send + Sync>() {}
    is_send_sync::<Slot<Q>>();
}

/// Check that `Slot<Q, MP>: 'static` as long as
/// `DB::DatabaseData: 'static`, which in turn implies that
/// `Q::Key: 'static`, `Q::Value: 'static`.
#[allow(dead_code)]
fn check_static<Q>()
where
    Q: Query + 'static,
    Q::Key: 'static,
    Q::Value: 'static,
{
    fn is_static<T: 'static>() {}
    is_static::<Slot<Q>>();
}

impl<Q> std::fmt::Debug for Slot<Q>
where
    Q: Query,
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{:?}({:?})", Q::default(), self.key)
    }
}
