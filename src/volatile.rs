use crate::runtime::Revision;
use crate::runtime::StampedValue;
use crate::CycleDetected;
use crate::Database;
use crate::Query;
use crate::QueryStorageOps;
use crate::QueryTable;
use log::debug;
use parking_lot::Mutex;
use rustc_hash::FxHashSet;
use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;
use std::hash::Hash;

/// Volatile Storage is just **always** considered dirty. Any time you
/// ask for the result of such a query, it is recomputed.
pub struct VolatileStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    /// We don't store the results of volatile queries,
    /// but we track in-progress set to detect cycles.
    in_progress: Mutex<FxHashSet<Q::Key>>,
}

impl<DB, Q> Default for VolatileStorage<DB, Q>
where
    Q: Query<DB>,
    DB: Database,
{
    fn default() -> Self {
        VolatileStorage {
            in_progress: Mutex::new(FxHashSet::default()),
        }
    }
}

impl<DB, Q> QueryStorageOps<DB, Q> for VolatileStorage<DB, Q>
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
        if !self.in_progress.lock().insert(key.clone()) {
            return Err(CycleDetected);
        }

        let (
            StampedValue {
                value,
                changed_at: _,
            },
            _inputs,
        ) = db
            .salsa_runtime()
            .execute_query_implementation::<Q>(db, descriptor, key);

        let was_in_progress = self.in_progress.lock().remove(key);
        assert!(was_in_progress);

        let revision_now = db.salsa_runtime().current_revision();

        db.salsa_runtime()
            .report_query_read(descriptor, revision_now);

        Ok(value)
    }

    fn maybe_changed_since(
        &self,
        _db: &'q DB,
        revision: Revision,
        key: &Q::Key,
        _descriptor: &DB::QueryDescriptor,
    ) -> bool {
        debug!(
            "{:?}({:?})::maybe_changed_since(revision={:?}) ==> true (volatile)",
            Q::default(),
            key,
            revision,
        );

        true
    }
}
