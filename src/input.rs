use crate::runtime::QueryDescriptorSet;
use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::CycleDetected;
use crate::MutQueryStorageOps;
use crate::Query;
use crate::QueryContext;
use crate::QueryDescriptor;
use crate::QueryStorageOps;
use crate::QueryTable;
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
pub struct InputStorage<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
    Q::Value: Default,
{
    map: RwLock<FxHashMap<Q::Key, StampedValue<Q::Value>>>,
}

impl<QC, Q> Default for InputStorage<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
    Q::Value: Default,
{
    fn default() -> Self {
        InputStorage {
            map: RwLock::new(FxHashMap::default()),
        }
    }
}

impl<QC, Q> InputStorage<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
    Q::Value: Default,
{
    fn read<'q>(
        &self,
        _query: &'q QC,
        key: &Q::Key,
        _descriptor: &QC::QueryDescriptor,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        {
            let map_read = self.map.read();
            if let Some(value) = map_read.get(key) {
                return Ok(value.clone());
            }
        }

        Ok(StampedValue {
            value: <Q::Value>::default(),
            changed_at: Revision::ZERO,
        })
    }
}

impl<QC, Q> QueryStorageOps<QC, Q> for InputStorage<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
    Q::Value: Default,
{
    fn try_fetch<'q>(
        &self,
        query: &'q QC,
        key: &Q::Key,
        descriptor: &QC::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        let StampedValue {
            value,
            changed_at: _,
        } = self.read(query, key, &descriptor)?;

        query.salsa_runtime().report_query_read(descriptor);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        _query: &'q QC,
        revision: Revision,
        key: &Q::Key,
        _descriptor: &QC::QueryDescriptor,
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
                .unwrap_or(Revision::ZERO)
        };

        changed_at > revision
    }
}

impl<QC, Q> MutQueryStorageOps<QC, Q> for InputStorage<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
    Q::Value: Default,
{
    fn set(&self, query: &QC, key: &Q::Key, value: Q::Value) {
        let key = key.clone();

        let mut map_write = self.map.write();

        // Do this *after* we acquire the lock, so that we are not
        // racing with somebody else to modify this same cell.
        // (Otherwise, someone else might write a *newer* revision
        // into the same cell while we block on the lock.)
        let changed_at = query.salsa_runtime().increment_revision();

        map_write.insert(key, StampedValue { value, changed_at });
    }
}
