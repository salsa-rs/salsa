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

#[derive(Default)]
pub struct TransparentStorage;

impl<QC, Q> QueryStorageOps<QC, Q> for TransparentStorage
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
        // FIXME: Should we even call `execute_query_implementation`
        // here? Or should we just call `Q::execute`, and maybe
        // separate out the `push`/`pop` operations.
        let descriptor = descriptor();
        Ok(query
            .salsa_runtime()
            .execute_query_implementation::<Q>(query, descriptor, key))
    }
}
