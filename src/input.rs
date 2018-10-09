use crate::runtime::ChangedAt;
use crate::runtime::QueryDescriptorSet;
use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::CycleDetected;
use crate::Database;
use crate::InputQueryStorageOps;
use crate::Query;
use crate::QueryDescriptor;
use crate::QueryStorageOps;
use crate::QueryTable;
use crate::UncheckedMutQueryStorageOps;
use log::debug;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;
use std::hash::Hash;

/// Input queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub struct InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    map: RwLock<FxHashMap<Q::Key, StampedValue<Q::Value>>>,
}

impl<DB, Q> Default for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    fn default() -> Self {
        InputStorage {
            map: RwLock::new(FxHashMap::default()),
        }
    }
}

impl<DB, Q> InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    fn read<'q>(
        &self,
        _db: &'q DB,
        key: &Q::Key,
        _descriptor: &DB::QueryDescriptor,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        {
            let map_read = self.map.read();
            if let Some(value) = map_read.get(key) {
                return Ok(value.clone());
            }
        }

        Ok(StampedValue {
            value: <Q::Value>::default(),
            changed_at: ChangedAt::Revision(Revision::ZERO),
        })
    }

    fn set_common(&self, key: &Q::Key, value: Q::Value, changed_at: impl FnOnce() -> ChangedAt) {
        let map = self.map.upgradable_read();

        // If this value was previously stored, check if this is an
        // *actual change* before we do anything.
        if let Some(old_value) = map.get(key) {
            if old_value.value == value {
                return;
            }
        }

        let key = key.clone();

        let mut map = RwLockUpgradableReadGuard::upgrade(map);

        // Do this *after* we acquire the lock, so that we are not
        // racing with somebody else to modify this same cell.
        // (Otherwise, someone else might write a *newer* revision
        // into the same cell while we block on the lock.)
        let changed_at = changed_at();
        let stamped_value = StampedValue { value, changed_at };

        match map.entry(key) {
            Entry::Occupied(mut entry) => {
                assert!(
                    !entry.get().changed_at.is_constant(),
                    "modifying `{:?}({:?})`, which was previously marked as constant (old value `{:?}`, new value `{:?}`)",
                    Q::default(),
                    entry.key(),
                    entry.get().value,
                    stamped_value.value,
                );

                entry.insert(stamped_value);
            }

            Entry::Vacant(entry) => {
                entry.insert(stamped_value);
            }
        }
    }
}

impl<DB, Q> QueryStorageOps<DB, Q> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    fn try_fetch(
        &self,
        db: &DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        let StampedValue { value, changed_at } = self.read(db, key, &descriptor)?;

        db.salsa_runtime().report_query_read(descriptor, changed_at);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        _db: &DB,
        revision: Revision,
        key: &Q::Key,
        _descriptor: &DB::QueryDescriptor,
    ) -> bool {
        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?})",
            Q::default(),
            key,
            revision,
        );

        let changed_at = {
            let map_read = self.map.read();
            map_read
                .get(key)
                .map(|v| v.changed_at)
                .unwrap_or(ChangedAt::Revision(Revision::ZERO))
        };

        changed_at.changed_since(revision)
    }
}

impl<DB, Q> InputQueryStorageOps<DB, Q> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    fn set(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        self.set_common(key, value, || {
            ChangedAt::Revision(db.salsa_runtime().increment_revision())
        });
    }

    fn set_constant(&self, _db: &DB, key: &Q::Key, value: Q::Value) {
        // FIXME. One weirdness: if this previously had a value, but
        // was not marked as constant, we will not "lower" the
        // `ChangedAt` rating with the code as it is.
        self.set_common(key, value, || ChangedAt::Constant);
    }
}

impl<DB, Q> UncheckedMutQueryStorageOps<DB, Q> for InputStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    fn set_unchecked(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        let key = key.clone();

        let mut map_write = self.map.write();

        // Unlike with `set`, here we use the **current revision** and
        // do not create a new one.
        let changed_at = ChangedAt::Revision(db.salsa_runtime().current_revision());

        map_write.insert(key, StampedValue { value, changed_at });
    }
}
