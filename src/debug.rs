//! Debugging APIs: these are meant for use when unit-testing or
//! debugging your application but aren't ordinarily needed.

use crate::plumbing;
use crate::plumbing::QueryStorageOps;
use crate::Database;
use crate::Query;
use crate::QueryTable;
use std::iter::FromIterator;

/// Additional methods on queries that can be used to "peek into"
/// their current state. These methods are meant for debugging and
/// observing the effects of garbage collection etc.
pub trait DebugQueryTable {
    /// Key of this query.
    type Key;

    /// True if salsa thinks that the value for `key` is a
    /// **constant**, meaning that it can never change, no matter what
    /// values the inputs take on from this point.
    fn is_constant(&self, key: Self::Key) -> bool;

    /// Get the (current) set of the keys in the query table.
    fn keys<C>(&self) -> C
    where
        C: FromIterator<Self::Key>;
}

impl<DB, Q> DebugQueryTable for QueryTable<'_, DB, Q>
where
    DB: plumbing::GetQueryTable<Q>,
    Q: Query<DB>,
{
    type Key = Q::Key;

    fn is_constant(&self, key: Q::Key) -> bool {
        self.storage.is_constant(self.db, &key)
    }

    fn keys<C>(&self) -> C
    where
        C: FromIterator<Q::Key>,
    {
        self.storage.keys(self.db)
    }
}
