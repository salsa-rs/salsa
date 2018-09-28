use crate::BaseQueryContext;
use crate::Query;
use crate::QueryTable;
use rustc_hash::FxHashMap;
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;
use std::hash::Hash;

// Total hack for now: assume that the Debug string
// for the key, combined with the type-id of the query,
// is sufficient for an equality comparison.

/// A simple-to-use query descriptor that is meant only for dumping
/// out cycle stack errors and not for any real recovery; also, not
/// especially efficient.
#[derive(PartialEq, Eq)]
crate struct DynDescriptor {
    type_id: TypeId,
    debug_string: String,
}

impl DynDescriptor {
    crate fn from_key<QC, Q>(_query: &QC, key: &Q::Key) -> DynDescriptor
    where
        QC: BaseQueryContext,
        Q: Query<QC>,
    {
        let type_id = TypeId::of::<Q>();
        let query = Q::default();
        let debug_string = format!("Query `{:?}` applied to `{:?}`", query, key);
        DynDescriptor {
            type_id,
            debug_string,
        }
    }
}

impl std::fmt::Debug for DynDescriptor {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(fmt, "{}", self.debug_string)
    }
}
