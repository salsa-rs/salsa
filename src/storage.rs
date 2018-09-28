use crate::BaseQueryContext;
use crate::CycleDetected;
use crate::Query;
use crate::QueryState;
use crate::QueryStorageOps;
use crate::QueryTable;
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
    map: RefCell<FxHashMap<Q::Key, QueryState<Q::Value>>>,
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
            let mut map = self.map.borrow_mut();
            match map.entry(key.clone()) {
                Entry::Occupied(entry) => {
                    return match entry.get() {
                        QueryState::InProgress => Err(CycleDetected),
                        QueryState::Memoized(value) => Ok(value.clone()),
                    };
                }
                Entry::Vacant(entry) => {
                    entry.insert(QueryState::InProgress);
                }
            }
        }

        // If we get here, the query is in progress, and we are the
        // ones tasked with finding its final value.
        let descriptor = descriptor();
        let value = query.execute_query_implementation::<Q>(descriptor, key);

        {
            let mut map = self.map.borrow_mut();
            let old_value = map.insert(key.clone(), QueryState::Memoized(value.clone()));
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
