use crate::runtime::QueryDescriptorSet;
use crate::runtime::Revision;
use crate::CycleDetected;
use crate::Query;
use crate::QueryContext;
use crate::QueryDescriptor;
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
    QC: QueryContext,
{
    map: RwLock<FxHashMap<Q::Key, QueryState<QC, Q>>>,
}

/// Defines the "current state" of query's memoized results.
#[derive(Debug)]
enum QueryState<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
{
    /// We are currently computing the result of this query; if we see
    /// this value in the table, it indeeds a cycle.
    InProgress,

    /// We have computed the query already, and here is the result.
    Memoized(Memoized<QC, Q>),
}

#[derive(Debug)]
struct Memoized<QC, Q>
where
    Q: Query<QC>,
    QC: QueryContext,
{
    value: Q::Value,

    inputs: QueryDescriptorSet<QC>,

    /// Last time that we checked our inputs to see if they have
    /// changed. If this is equal to the current revision, then the
    /// value is up to date. If not, we need to check our inputs and
    /// see if any of them have changed since our last check -- if so,
    /// we'll need to re-execute.
    verified_at: Revision,

    /// Last time that our value changed.
    changed_at: Revision,
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
        let revision_now = query.salsa_runtime().current_revision();

        let mut old_value = {
            let map_read = self.map.upgradable_read();
            if let Some(value) = map_read.get(key) {
                match value {
                    QueryState::InProgress => return Err(CycleDetected),
                    QueryState::Memoized(m) => {
                        if m.verified_at == revision_now {
                            return Ok(m.value.clone());
                        }
                    }
                }
            }

            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::InProgress)
        };

        // If we have an old-value, it *may* now be stale, since there
        // has been a new revision since the last time we checked. So,
        // first things first, let's walk over each of our previous
        // inputs and check whether they are out of date.
        if let Some(QueryState::Memoized(old_memo)) = &mut old_value {
            if old_memo
                .inputs
                .iter()
                .all(|old_input| !old_input.maybe_changed_since(old_memo.verified_at))
            {
                // If none of out inputs have changed since the last time we refreshed
                // our value, then our value must still be good. We'll just patch
                // the verified-at date and re-use it.
                old_memo.verified_at = revision_now;
                let value = old_memo.value.clone();

                let mut map_write = self.map.write();
                let placeholder = map_write.insert(key.clone(), old_value.unwrap());
                assert!(
                    match placeholder {
                        Some(QueryState::InProgress) => true,
                        _ => false,
                    },
                    "expected in-progress state",
                );
                return Ok(value);
            }
        }

        // Query was not previously executed or value is potentially
        // stale. Let's execute!
        let descriptor = descriptor();
        let (value, inputs) = query
            .salsa_runtime()
            .execute_query_implementation::<Q>(query, descriptor, key);

        // We assume that query is side-effect free -- that is, does
        // not mutate the "inputs" to the query system. Sanity check
        // that assumption here, at least to the best of our ability.
        assert_eq!(
            query.salsa_runtime().current_revision(),
            revision_now,
            "revision altered during query execution",
        );

        // If the new value is equal to the old one, then it didn't
        // really change, even if some of its inputs have. So we can
        // "backdate" our `changed_at` revision to be the same as the
        // old value.
        let mut changed_at = revision_now;
        if let Some(QueryState::Memoized(old_memo)) = &old_value {
            if old_memo.value == value {
                changed_at = old_memo.changed_at;
            }
        }

        {
            let mut map_write = self.map.write();

            let old_value = map_write.insert(
                key.clone(),
                QueryState::Memoized(Memoized {
                    value: value.clone(),
                    inputs,
                    verified_at: revision_now,
                    changed_at,
                }),
            );
            assert!(
                match old_value {
                    Some(QueryState::InProgress) => true,
                    _ => false,
                },
                "expected in-progress state",
            );
        }

        Ok(value)
    }
}
