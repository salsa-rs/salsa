use crate::CycleDetected;
use crate::Query;
use crate::QueryContext;
use crate::QueryStorageOps;
use crate::QueryTable;
use parking_lot::{RwLock};
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
    QC: QueryContext,
{
    map: RwLock<FxHashMap<Q::Key, Q::Value>>,
}

impl<QC, Q> Default for MemoizedStorage<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
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
    QC: QueryContext,
{
    fn try_fetch<'q>(
        &self,
        query: &'q QC,
        key: &Q::Key,
        descriptor: impl FnOnce() -> QC::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        {
            let map_read = self.map.read();
            if let Some(value) = map_read.get(key) {
                return Ok(value.clone());
            }
        }

        // If we get here, the query is in progress, and we are the
        // ones tasked with finding its final value.
        let descriptor = descriptor();
        let value = query
            .salsa_runtime()
            .execute_query_implementation::<Q>(query, descriptor, key)?;

        // Let's store the value! If some over thread has managed
        // to compute the value faster, use that and drop ours
        // (which should be equvalent) on the floor.
        let value = {
            let mut map_write = self.map.write();
            map_write.entry(key.clone())
                .or_insert_with(move || value)
                .clone()
        };
        Ok(value)
    }
}
