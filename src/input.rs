use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::hash::FxIndexMap;
use crate::plumbing::CycleRecoveryStrategy;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::revision::Revision;
use crate::runtime::StampedValue;
use crate::Database;
use crate::Query;
use crate::Runtime;
use crate::{DatabaseKeyIndex, QueryDb};
use indexmap::map::Entry;
use log::debug;
use parking_lot::RwLock;
use std::convert::TryFrom;

/// Input queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub struct InputStorage<Q>
where
    Q: Query,
{
    group_index: u16,
    slots: RwLock<FxIndexMap<Q::Key, Slot<Q>>>,
}

struct Slot<Q>
where
    Q: Query,
{
    key: Q::Key,
    database_key_index: DatabaseKeyIndex,

    /// Value for this input: initially, it is `Some`.
    ///
    /// If it is `None`, then the value was removed
    /// using `remove_input`.
    ///
    /// Note that the slot is *never* removed, so as to preserve
    /// the `DatabaseKeyIndex` values.
    ///
    /// Impl note: We store an `Option<StampedValue<V>>`
    /// instead of a `StampedValue<Option<V>>` for two reasons.
    /// One, it corresponds to "data never existed in the first place",
    /// and two, it's more efficient, since the compiler can make
    /// use of the revisions in the `StampedValue` as a niche to avoid
    /// an extra word. (See the `assert_size_of` test below.)
    stamped_value: RwLock<Option<StampedValue<Q::Value>>>,
}

#[test]
fn assert_size_of() {
    assert_eq!(
        std::mem::size_of::<RwLock<Option<StampedValue<u32>>>>(),
        std::mem::size_of::<RwLock<StampedValue<u32>>>(),
    );
}

impl<Q> std::panic::RefUnwindSafe for InputStorage<Q>
where
    Q: Query,
    Q::Key: std::panic::RefUnwindSafe,
    Q::Value: std::panic::RefUnwindSafe,
{
}

impl<Q> QueryStorageOps<Q> for InputStorage<Q>
where
    Q: Query,
{
    const CYCLE_STRATEGY: crate::plumbing::CycleRecoveryStrategy = CycleRecoveryStrategy::Panic;

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

    fn maybe_changed_after(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> bool {
        assert_eq!(input.group_index, self.group_index);
        assert_eq!(input.query_index, Q::QUERY_INDEX);
        debug_assert!(revision < db.salsa_runtime().current_revision());
        let slots = self.slots.read();
        let (_, slot) = slots.get_index(input.key_index as usize).unwrap();
        slot.maybe_changed_after(db, revision)
    }

    fn fetch(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Q::Value {
        db.unwind_if_cancelled();

        let slots = self.slots.read();
        let slot = slots
            .get(key)
            .unwrap_or_else(|| panic!("no value set for {:?}({:?})", Q::default(), key));

        let value = slot.stamped_value.read().clone();
        match value {
            Some(StampedValue {
                value,
                durability,
                changed_at,
            }) => {
                db.salsa_runtime()
                    .report_query_read_and_unwind_if_cycle_resulted(
                        slot.database_key_index,
                        durability,
                        changed_at,
                    );

                value
            }

            None => {
                panic!("value removed for {:?}({:?})", Q::default(), key)
            }
        }
    }

    fn durability(&self, _db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Durability {
        let slots = self.slots.read();
        match slots.get(key) {
            Some(slot) => match &*slot.stamped_value.read() {
                Some(v) => v.durability,
                None => Durability::LOW, // removed
            },
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
                let value = (*slot.stamped_value.read())
                    .as_ref()
                    .map(|stamped_value| stamped_value.value.clone());
                TableEntry::new(slot.key.clone(), value)
            })
            .collect()
    }
}

impl<Q> Slot<Q>
where
    Q: Query,
{
    fn maybe_changed_after(&self, _db: &<Q as QueryDb<'_>>::DynDb, revision: Revision) -> bool {
        debug!(
            "maybe_changed_after(slot={:?}, revision={:?})",
            self, revision,
        );

        match &*self.stamped_value.read() {
            Some(stamped_value) => {
                let changed_at = stamped_value.changed_at;

                debug!("maybe_changed_after: changed_at = {:?}", changed_at);

                changed_at > revision
            }

            None => {
                // treat a removed input as always having changed
                true
            }
        }
    }
}

impl<Q> QueryStorageMassOps for InputStorage<Q>
where
    Q: Query,
{
    fn purge(&self) {
        *self.slots.write() = Default::default();
    }
}

impl<Q> InputQueryStorageOps<Q> for InputStorage<Q>
where
    Q: Query,
{
    fn set(&self, runtime: &mut Runtime, key: &Q::Key, value: Q::Value, durability: Durability) {
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
        runtime.with_incremented_revision(|next_revision| {
            let mut slots = self.slots.write();

            // Do this *after* we acquire the lock, so that we are not
            // racing with somebody else to modify this same cell.
            // (Otherwise, someone else might write a *newer* revision
            // into the same cell while we block on the lock.)
            let stamped_value = StampedValue {
                value,
                durability,
                changed_at: next_revision,
            };

            match slots.entry(key.clone()) {
                Entry::Occupied(entry) => {
                    let mut slot_stamped_value = entry.get().stamped_value.write();
                    match &mut *slot_stamped_value {
                        Some(slot_stamped_value) => {
                            // Modifying an existing value that has not been removed.
                            let old_durability = slot_stamped_value.durability;
                            *slot_stamped_value = stamped_value;
                            Some(old_durability)
                        }

                        None => {
                            // Overwriting a removed value: this is the same as inserting a new value,
                            // it doesn't modify any existing data (the remove did that).
                            *slot_stamped_value = Some(stamped_value);
                            None
                        }
                    }
                }

                Entry::Vacant(entry) => {
                    let key_index = u32::try_from(entry.index()).unwrap();
                    let database_key_index = DatabaseKeyIndex {
                        group_index: self.group_index,
                        query_index: Q::QUERY_INDEX,
                        key_index,
                    };
                    entry.insert(Slot {
                        key: key.clone(),
                        database_key_index,
                        stamped_value: RwLock::new(Some(stamped_value)),
                    });
                    None
                }
            }
        });
    }

    fn remove(&self, runtime: &mut Runtime, key: &<Q as Query>::Key) -> <Q as Query>::Value {
        let mut value = None;
        runtime.with_incremented_revision(&mut |_| {
            let mut slots = self.slots.write();
            let slot = slots.get_mut(key)?;

            if let Some(slot_stamped_value) = slot.stamped_value.get_mut().take() {
                value = Some(slot_stamped_value.value);
                Some(slot_stamped_value.durability)
            } else {
                None
            }
        });

        value.unwrap_or_else(|| panic!("no value set for {:?}({:?})", Q::default(), key))
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
