use crate::debug::TableEntry;
use crate::dependency::DatabaseSlot;
use crate::plumbing::CycleDetected;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::runtime::ChangedAt;
use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::Database;
use crate::Event;
use crate::EventKind;
use crate::Query;
use crate::SweepStrategy;
use log::debug;
use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

/// Input queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub struct InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    slots: RwLock<FxHashMap<Q::Key, Arc<Slot<DB, Q>>>>,
}

struct Slot<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    key: Q::Key,
    stamped_value: RwLock<StampedValue<Q::Value>>,
}

impl<DB, Q> std::panic::RefUnwindSafe for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: std::panic::RefUnwindSafe,
{
}

impl<DB, Q> Default for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn default() -> Self {
        InputStorage {
            slots: Default::default(),
        }
    }
}

struct IsConstant(bool);

impl<DB, Q> InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn slot(&self, key: &Q::Key) -> Option<Arc<Slot<DB, Q>>> {
        self.slots.read().get(key).cloned()
    }

    fn set_common(
        &self,
        db: &DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
        value: Q::Value,
        is_constant: IsConstant,
    ) {
        // The value is changing, so even if we are setting this to a
        // constant, we still need a new revision.
        //
        // CAREFUL: This will block until the global revision lock can
        // be acquired. If there are still queries executing, they may
        // need to read from this input. Therefore, we wait to acquire
        // the lock on `map` until we also hold the global query write
        // lock.
        db.salsa_runtime().with_incremented_revision(|next_revision| {
            let mut slots = self.slots.write();

            db.salsa_event(|| Event {
                runtime_id: db.salsa_runtime().id(),
                kind: EventKind::WillChangeInputValue {
                    database_key: database_key.clone(),
                },
            });

            // Do this *after* we acquire the lock, so that we are not
            // racing with somebody else to modify this same cell.
            // (Otherwise, someone else might write a *newer* revision
            // into the same cell while we block on the lock.)
            let changed_at = ChangedAt {
                is_constant: is_constant.0,
                revision: next_revision,
            };

            let stamped_value = StampedValue { value, changed_at };

            match slots.entry(key.clone()) {
                Entry::Occupied(entry) => {
                    let mut slot_stamped_value = entry.get().stamped_value.write();

                    assert!(
                        !slot_stamped_value.changed_at.is_constant,
                        "modifying `{:?}({:?})`, which was previously marked as constant (old value `{:?}`, new value `{:?}`)",
                        Q::default(),
                        entry.key(),
                        slot_stamped_value.value,
                        stamped_value.value,
                    );

                    *slot_stamped_value = stamped_value;
                }

                Entry::Vacant(entry) => {
                    entry.insert(Arc::new(Slot {
                        key: key.clone(),
                        stamped_value: RwLock::new(stamped_value),
                    }));
                }
            }
        });
    }
}

impl<DB, Q> QueryStorageOps<DB, Q> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn try_fetch(&self, db: &DB, key: &Q::Key) -> Result<Q::Value, CycleDetected> {
        let slot = match self.slot(key) {
            Some(s) => s.clone(),
            None => panic!("no value set for {:?}({:?})", Q::default(), key),
        };

        let StampedValue { value, changed_at } = slot.stamped_value.read().clone();

        db.salsa_runtime().report_query_read(slot, changed_at);

        Ok(value)
    }

    fn is_constant(&self, _db: &DB, key: &Q::Key) -> bool {
        self.slot(key)
            .map(|slot| slot.stamped_value.read().changed_at.is_constant)
            .unwrap_or(false)
    }

    fn entries<C>(&self, _db: &DB) -> C
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

impl<DB, Q> QueryStorageMassOps<DB> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn sweep(&self, _db: &DB, _strategy: SweepStrategy) {}
}

impl<DB, Q> InputQueryStorageOps<DB, Q> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn set(&self, db: &DB, key: &Q::Key, database_key: &DB::DatabaseKey, value: Q::Value) {
        log::debug!("{:?}({:?}) = {:?}", Q::default(), key, value);

        self.set_common(db, key, database_key, value, IsConstant(false))
    }

    fn set_constant(&self, db: &DB, key: &Q::Key, database_key: &DB::DatabaseKey, value: Q::Value) {
        log::debug!("{:?}({:?}) = {:?}", Q::default(), key, value);

        self.set_common(db, key, database_key, value, IsConstant(true))
    }
}

impl<DB, Q> DatabaseSlot<DB> for Slot<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn maybe_changed_since(&self, _db: &DB, revision: Revision) -> bool {
        debug!(
            "maybe_changed_since(slot={:?}, revision={:?})",
            self, revision,
        );

        let changed_at = self.stamped_value.read().changed_at;

        debug!("maybe_changed_since: changed_at = {:?}", changed_at);

        changed_at.changed_since(revision)
    }
}

impl<DB, Q> std::fmt::Debug for Slot<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{:?}({:?})", Q::default(), self.key)
    }
}
