use crate::runtime::Revision;
use crate::CycleDetected;
use crate::Query;
use crate::QueryContext;
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

/// Volatile Storage is just **always** considered dirty. Any time you
/// ask for the result of such a query, it is recomputed.
#[derive(Default)]
pub struct VolatileStorage;

impl<QC, Q> QueryStorageOps<QC, Q> for VolatileStorage
where
    Q: Query<QC>,
    QC: QueryContext,
{
    fn try_fetch<'q>(
        &self,
        query: &'q QC,
        key: &Q::Key,
        descriptor: &QC::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected> {
        // FIXME: Should we even call `execute_query_implementation`
        // here? Or should we just call `Q::execute`, and maybe
        // separate out the `push`/`pop` operations.
        let (value, _inputs) = query
            .salsa_runtime()
            .execute_query_implementation::<Q>(query, descriptor, key);

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
            "{:?}({:?})::maybe_changed_since(revision={:?}) ==> true (volatile)",
            Q::default(),
            key,
            revision,
        );

        true
    }
}
