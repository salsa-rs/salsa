use crate::BaseQueryContext;
use crate::CycleDetected;
use crate::Query;
use crate::QueryStorageOps;
use crate::QueryTable;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;
use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;
use std::hash::Hash;

// The master implementation that knits together all the queries
// contains a certain amount of boilerplate. This file aims to
// reduce that.

pub struct MemoizedStorage<QC, Q>
where
    Q: Query<QC>,
    QC: BaseQueryContext,
{
    map: RwLock<FxHashMap<Q::Key, QueryState<Q::Value>>>,
}

/// Defines the "current state" of query's memoized results.
#[derive(Debug)]
pub enum QueryState<V> {
    /// We are currently computing the result of this query; if we see
    /// this value in the table, it indeeds a cycle.
    InProgress,

    /// We have computed the query already, and here is the result.
    Memoized(V),
}

impl<QC, Q> Default for MemoizedStorage<QC, Q>
where
    Q: Query<QC>,
    QC: BaseQueryContext,
{
    fn default() -> Self {
        MemoizedStorage {
            map: RwLock::new(FxHashMap::default()),
        }
    }
}

impl<QC, Q> QueryStorageOps<QC, Q> for MemoizedStorage<QC, Q>
where
    Q: Query<QC>,
    QC: BaseQueryContext,
{
    fn try_fetch<'q>(
        &self,
        query: &'q QC,
        key: &Q::Key,
        descriptor: impl FnOnce() -> QC::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        {
            let map_read = self.map.upgradable_read();
            if let Some(value) = map_read.get(key) {
                return match value {
                    QueryState::InProgress => Err(CycleDetected),
                    QueryState::Memoized(value) => Ok(value.clone()),
                };
            }

            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::InProgress);
        }

        // If we get here, the query is in progress, and we are the
        // ones tasked with finding its final value.
        let descriptor = descriptor();
        let value = query.execute_query_implementation::<Q>(descriptor, key);

        {
            let mut map_write = self.map.write();
            let old_value = map_write.insert(key.clone(), QueryState::Memoized(value.clone()));
            assert!(
                match old_value {
                    Some(QueryState::InProgress) => true,
                    _ => false,
                },
                "expected in-progress state, not {:?}",
                old_value
            );
        }

        Ok(value)
    }
}
