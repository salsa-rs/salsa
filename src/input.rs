use crate::debug::TableEntry;
use crate::dependency::DatabaseSlot;
use crate::durability::Durability;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::revision::Revision;
use crate::runtime::StampedValue;
use crate::CycleError;
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

impl<DB, Q> InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn slot(&self, key: &Q::Key) -> Option<Arc<Slot<DB, Q>>> {
        self.slots.read().get(key).cloned()
    }
}

#[async_trait::async_trait]
impl<DB, Q> QueryStorageOps<DB, Q> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    async fn try_fetch(
        &self,
        db: &mut DB,
        key: &Q::Key,
    ) -> Result<Q::Value, CycleError<DB::DatabaseKey>> {
        let slot = self
            .slot(key)
            .unwrap_or_else(|| panic!("no value set for {:?}({:?})", Q::default(), key));

        let StampedValue {
            value,
            durability,
            changed_at,
        } = slot.stamped_value.read().clone();

        db.salsa_runtime_mut()
            .report_query_read(slot, durability, changed_at);

        Ok(value)
    }

    fn peek(&self, db: &DB, key: &Q::Key) -> Option<Q::Value> {
        let slot = self.slot(key)?;

        let StampedValue { value, .. } = slot.stamped_value.read().clone();

        Some(value)
    }

    fn durability(&self, _db: &DB, key: &Q::Key) -> Durability {
        match self.slot(key) {
            Some(slot) => slot.stamped_value.read().durability,
            None => panic!("no value set for {:?}({:?})", Q::default(), key),
        }
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
    fn set(
        &self,
        db: &mut DB,
        key: &Q::Key,
        database_key: &DB::DatabaseKey,
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

        db.salsa_event(|| Event {
            runtime_id: db.salsa_runtime().id(),
            kind: EventKind::WillChangeInputValue {
                database_key: database_key.clone(),
            },
        });

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
        db.salsa_runtime_mut().with_incremented_revision(|guard| {
            let mut slots = self.slots.write();

            // Do this *after* we acquire the lock, so that we are not
            // racing with somebody else to modify this same cell.
            // (Otherwise, someone else might write a *newer* revision
            // into the same cell while we block on the lock.)
            let stamped_value = StampedValue {
                value,
                durability,
                changed_at: guard.new_revision(),
            };

            match slots.entry(key.clone()) {
                Entry::Occupied(entry) => {
                    let mut slot_stamped_value = entry.get().stamped_value.write();
                    guard.mark_durability_as_changed(slot_stamped_value.durability);
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

// Unsafe proof obligation: `Slot<DB, Q>` is Send + Sync if the query
// key/value is Send + Sync (also, that we introduce no
// references). These are tested by the `check_send_sync` and
// `check_static` helpers below.
#[async_trait::async_trait]
unsafe impl<DB, Q> DatabaseSlot<DB> for Slot<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    async fn maybe_changed_since(&self, _db: &mut DB, revision: Revision) -> bool {
        debug!(
            "maybe_changed_since(slot={:?}, revision={:?})",
            self, revision,
        );

        let changed_at = self.stamped_value.read().changed_at;

        debug!("maybe_changed_since: changed_at = {:?}", changed_at);

        changed_at > revision
    }
}

/// Check that `Slot<DB, Q, MP>: Send + Sync` as long as
/// `DB::DatabaseData: Send + Sync`, which in turn implies that
/// `Q::Key: Send + Sync`, `Q::Value: Send + Sync`.
#[allow(dead_code)]
fn check_send_sync<DB, Q>()
where
    Q: Query<DB>,
    DB: Database,
    DB::DatabaseData: Send + Sync,
    Q::Key: Send + Sync,
    Q::Value: Send + Sync,
{
    fn is_send_sync<T: Send + Sync>() {}
    is_send_sync::<Slot<DB, Q>>();
}

/// Check that `Slot<DB, Q, MP>: 'static` as long as
/// `DB::DatabaseData: 'static`, which in turn implies that
/// `Q::Key: 'static`, `Q::Value: 'static`.
#[allow(dead_code)]
fn check_static<DB, Q>()
where
    Q: Query<DB>,
    DB: Database,
    DB: 'static,
    DB::DatabaseData: 'static,
    Q::Key: 'static,
    Q::Value: 'static,
{
    fn is_static<T: 'static>() {}
    is_static::<Slot<DB, Q>>();
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
