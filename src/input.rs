use crate::plumbing::CycleDetected;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::plumbing::UncheckedMutQueryStorageOps;
use crate::runtime::ChangedAt;
use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::Database;
use crate::Query;
use crate::SweepStrategy;
use log::debug;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use std::collections::hash_map::Entry;
use std::marker::PhantomData;

/// Input queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub struct InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
{
    map: RwLock<FxHashMap<Q::Key, StampedValue<Q::Value>>>,
    input_policy: PhantomData<IP>,
}

pub trait InputPolicy<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn compare_values() -> bool {
        false
    }

    fn compare_value(_old_value: &Q::Value, _new_value: &Q::Value) -> bool {
        panic!("should never be asked to compare values")
    }

    fn missing_value(key: &Q::Key) -> Q::Value {
        panic!("no value set for {:?}({:?})", Q::default(), key)
    }
}

/// The default policy for inputs:
///
/// - Each time a new value is set, trigger a new revision.
/// - On an attempt access a value that is not yet set, panic.
pub enum ExplicitInputPolicy {}
impl<DB, Q> InputPolicy<DB, Q> for ExplicitInputPolicy
where
    Q: Query<DB>,
    DB: Database,
{
}

/// Alternative policy for inputs:
///
/// - Each time a new value is set, trigger a new revision.
/// - On an attempt access a value that is not yet set, use `Default::default` to find
///   the value.
///
/// Requires that `Q::Value` implements the `Default` trait.
pub enum DefaultValueInputPolicy {}
impl<DB, Q> InputPolicy<DB, Q> for DefaultValueInputPolicy
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default,
{
    fn missing_value(_key: &Q::Key) -> Q::Value {
        <Q::Value>::default()
    }
}

/// Alternative policy for inputs:
///
/// - Each time a new value is set, trigger a new revision
///   only if it is not equal to the old value.
/// - On an attempt access a value that is not yet set, panic.
///
/// Requires that `Q::Value` implements the `Eq` trait.
pub enum EqValueInputPolicy {}
impl<DB, Q> InputPolicy<DB, Q> for EqValueInputPolicy
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Eq,
{
    fn compare_values() -> bool {
        true
    }

    fn compare_value(old_value: &Q::Value, new_value: &Q::Value) -> bool {
        old_value == new_value
    }
}

/// Alternative policy for inputs:
///
/// - Each time a new value is set, trigger a new revision
///   only if it is not equal to the old value.
/// - On an attempt access a value that is not yet set, use `Default::default`.
///
/// Requires that `Q::Value` implements the `Eq` trait.
pub enum DefaultEqValueInputPolicy {}
impl<DB, Q> InputPolicy<DB, Q> for DefaultEqValueInputPolicy
where
    Q: Query<DB>,
    DB: Database,
    Q::Value: Default + Eq,
{
    fn compare_values() -> bool {
        true
    }

    fn compare_value(old_value: &Q::Value, new_value: &Q::Value) -> bool {
        old_value == new_value
    }

    fn missing_value(_key: &Q::Key) -> Q::Value {
        <Q::Value>::default()
    }
}

impl<DB, Q, IP> Default for InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
{
    fn default() -> Self {
        InputStorage {
            map: RwLock::new(FxHashMap::default()),
            input_policy: PhantomData,
        }
    }
}

struct IsConstant(bool);

impl<DB, Q, IP> InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
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

        let value = IP::missing_value(key);

        Ok(StampedValue {
            value: value,
            changed_at: ChangedAt {
                is_constant: false,
                revision: Revision::ZERO,
            },
        })
    }

    fn set_common(&self, db: &DB, key: &Q::Key, value: Q::Value, is_constant: IsConstant) {
        let map = self.map.upgradable_read();

        if IP::compare_values() {
            if let Some(old_value) = map.get(key) {
                if IP::compare_value(&old_value.value, &value) {
                    // If the value did not change, but it is now
                    // considered constant, we can just update
                    // `changed_at`. We don't have to trigger a new
                    // revision for this case: all the derived values are
                    // still intact, they just have conservative
                    // dependencies. The next revision, they may wind up
                    // with something more precise.
                    if is_constant.0 && !old_value.changed_at.is_constant {
                        let mut map = RwLockUpgradableReadGuard::upgrade(map);
                        let old_value = map.get_mut(key).unwrap();
                        old_value.changed_at.is_constant = true;
                    }

                    return;
                }
            }
        }

        let key = key.clone();

        // The value is changing, so even if we are setting this to a
        // constant, we still need a new revision.
        //
        // CAREFUL: This will block until the global revision lock can
        // be acquired. If there are still queries executing, they may
        // need to read from this input. Therefore, we do not upgrade
        // our lock (which would prevent them from reading) until
        // `increment_revision` has finished.
        let next_revision = db.salsa_runtime().increment_revision();

        let mut map = RwLockUpgradableReadGuard::upgrade(map);

        // Do this *after* we acquire the lock, so that we are not
        // racing with somebody else to modify this same cell.
        // (Otherwise, someone else might write a *newer* revision
        // into the same cell while we block on the lock.)
        let changed_at = ChangedAt {
            is_constant: is_constant.0,
            revision: next_revision,
        };

        let stamped_value = StampedValue { value, changed_at };

        match map.entry(key) {
            Entry::Occupied(mut entry) => {
                assert!(
                    !entry.get().changed_at.is_constant,
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

impl<DB, Q, IP> QueryStorageOps<DB, Q> for InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
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
                .unwrap_or(ChangedAt {
                    is_constant: false,
                    revision: Revision::ZERO,
                })
        };

        debug!(
            "{:?}({:?}): changed_at = {:?}",
            Q::default(),
            key,
            changed_at,
        );

        changed_at.changed_since(revision)
    }

    fn is_constant(&self, _db: &DB, key: &Q::Key) -> bool {
        let map_read = self.map.read();
        map_read
            .get(key)
            .map(|v| v.changed_at.is_constant)
            .unwrap_or(false)
    }

    fn keys<C>(&self, _db: &DB) -> C
    where
        C: std::iter::FromIterator<Q::Key>,
    {
        let map = self.map.read();
        map.keys().cloned().collect()
    }
}

impl<DB, Q, IP> QueryStorageMassOps<DB> for InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
{
    fn sweep(&self, _db: &DB, _strategy: SweepStrategy) {}
}

impl<DB, Q, IP> InputQueryStorageOps<DB, Q> for InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
{
    fn set(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        log::debug!("{:?}({:?}) = {:?}", Q::default(), key, value);

        self.set_common(db, key, value, IsConstant(false))
    }

    fn set_constant(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        log::debug!("{:?}({:?}) = {:?}", Q::default(), key, value);

        self.set_common(db, key, value, IsConstant(true))
    }
}

impl<DB, Q, IP> UncheckedMutQueryStorageOps<DB, Q> for InputStorage<DB, Q, IP>
where
    Q: Query<DB>,
    DB: Database,
    IP: InputPolicy<DB, Q>,
{
    fn set_unchecked(&self, db: &DB, key: &Q::Key, value: Q::Value) {
        let key = key.clone();

        let mut map_write = self.map.write();

        // Unlike with `set`, here we use the **current revision** and
        // do not create a new one.
        let changed_at = ChangedAt {
            is_constant: false,
            revision: db.salsa_runtime().current_revision(),
        };

        map_write.insert(key, StampedValue { value, changed_at });
    }
}
