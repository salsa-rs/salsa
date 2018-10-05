use crate::runtime::QueryDescriptorSet;
use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::CycleDetected;
use crate::Database;
use crate::Query;
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

/// Memoized queries store the result plus a list of the other queries
/// that they invoked. This means we can avoid recomputing them when
/// none of those inputs have changed.
pub struct MemoizedStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    map: RwLock<FxHashMap<Q::Key, QueryState<DB, Q>>>,
}

/// Defines the "current state" of query's memoized results.
enum QueryState<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    /// We are currently computing the result of this query; if we see
    /// this value in the table, it indeeds a cycle.
    InProgress,

    /// We have computed the query already, and here is the result.
    Memoized(Memo<DB, Q>),
}

struct Memo<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    stamped_value: StampedValue<Q::Value>,

    inputs: QueryDescriptorSet<DB>,

    /// Last time that we checked our inputs to see if they have
    /// changed. If this is equal to the current revision, then the
    /// value is up to date. If not, we need to check our inputs and
    /// see if any of them have changed since our last check -- if so,
    /// we'll need to re-execute.
    verified_at: Revision,
}

impl<DB, Q> Default for MemoizedStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn default() -> Self {
        MemoizedStorage {
            map: RwLock::new(FxHashMap::default()),
        }
    }
}

impl<DB, Q> MemoizedStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn read(
        &self,
        db: &DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> Result<StampedValue<Q::Value>, CycleDetected> {
        let revision_now = db.salsa_runtime().current_revision();

        debug!(
            "{:?}({:?}): invoked at {:?}",
            Q::default(),
            key,
            revision_now,
        );

        let mut old_value = {
            let map_read = self.map.upgradable_read();
            if let Some(value) = map_read.get(key) {
                match value {
                    QueryState::InProgress => return Err(CycleDetected),
                    QueryState::Memoized(m) => {
                        debug!(
                            "{:?}({:?}): found memoized value verified_at={:?}",
                            Q::default(),
                            key,
                            m.verified_at,
                        );

                        if m.verified_at == revision_now {
                            debug!(
                                "{:?}({:?}): returning memoized value (changed_at={:?})",
                                Q::default(),
                                key,
                                m.stamped_value.changed_at,
                            );

                            return Ok(m.stamped_value.clone());
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
            if old_memo.inputs.iter().all(|old_input| {
                !old_input.maybe_changed_since(db, old_memo.stamped_value.changed_at)
            }) {
                debug!("{:?}({:?}): inputs still valid", Q::default(), key);

                // If none of out inputs have changed since the last time we refreshed
                // our value, then our value must still be good. We'll just patch
                // the verified-at date and re-use it.
                old_memo.verified_at = revision_now;
                let stamped_value = old_memo.stamped_value.clone();

                let mut map_write = self.map.write();
                self.overwrite_placeholder(&mut map_write, key, old_value.unwrap());
                return Ok(stamped_value);
            }
        }

        // Query was not previously executed or value is potentially
        // stale. Let's execute!
        let (mut stamped_value, inputs) = db
            .salsa_runtime()
            .execute_query_implementation::<Q>(db, descriptor, key);

        // We assume that query is side-effect free -- that is, does
        // not mutate the "inputs" to the query system. Sanity check
        // that assumption here, at least to the best of our ability.
        assert_eq!(
            db.salsa_runtime().current_revision(),
            revision_now,
            "revision altered during query execution",
        );

        // If the new value is equal to the old one, then it didn't
        // really change, even if some of its inputs have. So we can
        // "backdate" its `changed_at` revision to be the same as the
        // old value.
        if let Some(QueryState::Memoized(old_memo)) = &old_value {
            if old_memo.stamped_value.value == stamped_value.value {
                assert!(old_memo.stamped_value.changed_at <= stamped_value.changed_at);
                stamped_value.changed_at = old_memo.stamped_value.changed_at;
            }
        }

        {
            let mut map_write = self.map.write();
            self.overwrite_placeholder(
                &mut map_write,
                key,
                QueryState::Memoized(Memo {
                    stamped_value: stamped_value.clone(),
                    inputs,
                    verified_at: revision_now,
                }),
            );
        }

        Ok(stamped_value)
    }

    fn overwrite_placeholder(
        &self,
        map_write: &mut FxHashMap<Q::Key, QueryState<DB, Q>>,
        key: &Q::Key,
        value: QueryState<DB, Q>,
    ) {
        let old_value = map_write.insert(key.clone(), value);
        assert!(
            match old_value {
                Some(QueryState::InProgress) => true,
                _ => false,
            },
            "expected in-progress state",
        );
    }
}

impl<DB, Q> QueryStorageOps<DB, Q> for MemoizedStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn try_fetch<'q>(
        &self,
        db: &'q DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        let StampedValue { value, changed_at } = self.read(db, key, &descriptor)?;

        db.salsa_runtime().report_query_read(descriptor, changed_at);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        db: &'q DB,
        revision: Revision,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> bool {
        let revision_now = db.salsa_runtime().current_revision();

        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?}, revision_now={:?})",
            Q::default(),
            key,
            revision,
            revision_now,
        );

        // Check for the case where we have no cache entry, or our cache
        // entry is up to date (common case):
        {
            let map_read = self.map.read();
            match map_read.get(key) {
                None | Some(QueryState::InProgress) => return true,
                Some(QueryState::Memoized(memo)) => {
                    if memo.verified_at >= revision_now {
                        return memo.stamped_value.changed_at > revision;
                    }
                }
            }
        }

        // Otherwise fall back to the full read to compute the result.
        match self.read(db, key, descriptor) {
            Ok(v) => v.changed_at > revision,
            Err(CycleDetected) => true,
        }
    }
}
