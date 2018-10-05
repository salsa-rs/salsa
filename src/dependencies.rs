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

/// "Dependency" queries just track their dependencies and not the
/// actual value (which they produce on demand). This lessens the
/// storage requirements.
pub struct DependencyStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    map: RwLock<FxHashMap<Q::Key, QueryState<DB>>>,
}

/// Defines the "current state" of query's memoized results.
enum QueryState<DB>
where
    DB: Database,
{
    /// We are currently computing the result of this query; if we see
    /// this value in the table, it indeeds a cycle.
    InProgress,

    /// We have computed the query already, and here is the result.
    Memoized(Memo<DB>),
}

struct Memo<DB>
where
    DB: Database,
{
    inputs: QueryDescriptorSet<DB>,

    /// Last time that we checked our inputs to see if they have
    /// changed. If this is equal to the current revision, then the
    /// value is up to date. If not, we need to check our inputs and
    /// see if any of them have changed since our last check -- if so,
    /// we'll need to re-execute.
    verified_at: Revision,

    /// Last time that our value changed.
    changed_at: Revision,
}

impl<DB, Q> Default for DependencyStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn default() -> Self {
        DependencyStorage {
            map: RwLock::new(FxHashMap::default()),
        }
    }
}

impl<DB, Q> DependencyStorage<DB, Q>
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

        {
            let map_read = self.map.upgradable_read();
            if let Some(value) = map_read.get(key) {
                match value {
                    QueryState::InProgress => return Err(CycleDetected),
                    QueryState::Memoized(_) => {}
                }
            }

            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::InProgress);
        }

        // Note that, unlike with a memoized query, we must always
        // re-execute.
        let (stamped_value, inputs) = db
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

        {
            let mut map_write = self.map.write();

            let old_value = map_write.insert(
                key.clone(),
                QueryState::Memoized(Memo {
                    inputs,
                    verified_at: revision_now,
                    changed_at: stamped_value.changed_at,
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

        Ok(stamped_value)
    }

    fn overwrite_placeholder(
        &self,
        map_write: &mut FxHashMap<Q::Key, QueryState<DB>>,
        key: &Q::Key,
        value: Option<QueryState<DB>>,
    ) {
        let old_value = if let Some(v) = value {
            map_write.insert(key.clone(), v)
        } else {
            map_write.remove(key)
        };

        assert!(
            match old_value {
                Some(QueryState::InProgress) => true,
                _ => false,
            },
            "expected in-progress state",
        );
    }
}

impl<DB, Q> QueryStorageOps<DB, Q> for DependencyStorage<DB, Q>
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
        _descriptor: &DB::QueryDescriptor,
    ) -> bool {
        let revision_now = db.salsa_runtime().current_revision();

        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?}, revision_now={:?})",
            Q::default(),
            key,
            revision,
            revision_now,
        );

        let value = {
            let map_read = self.map.upgradable_read();
            match map_read.get(key) {
                None | Some(QueryState::InProgress) => return true,
                Some(QueryState::Memoized(memo)) => {
                    // If our memo is still up to date, then check if we've
                    // changed since the revision.
                    if memo.verified_at == revision_now {
                        return memo.changed_at > revision;
                    }
                }
            }

            let mut map_write = RwLockUpgradableReadGuard::upgrade(map_read);
            map_write.insert(key.clone(), QueryState::InProgress)
        };

        // Otherwise, walk the inputs we had and check them. Note that
        // we don't want to hold the lock while we do this.
        let mut memo = match value {
            Some(QueryState::Memoized(memo)) => memo,
            _ => unreachable!(),
        };

        if memo
            .inputs
            .iter()
            .all(|old_input| !old_input.maybe_changed_since(db, memo.verified_at))
        {
            memo.verified_at = revision_now;
            self.overwrite_placeholder(
                &mut self.map.write(),
                key,
                Some(QueryState::Memoized(memo)),
            );
            return false;
        }

        // Just remove the existing entry. It's out of date.
        self.overwrite_placeholder(&mut self.map.write(), key, None);

        true
    }
}
