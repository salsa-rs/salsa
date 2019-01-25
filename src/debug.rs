//! Debugging APIs: these are meant for use when unit-testing or
//! debugging your application but aren't ordinarily needed.

use crate::plumbing;
use crate::plumbing::QueryStorageOps;
use crate::Query;
use crate::QueryTable;
use std::iter::FromIterator;

/// Additional methods on queries that can be used to "peek into"
/// their current state. These methods are meant for debugging and
/// observing the effects of garbage collection etc.
pub trait DebugQueryTable {
    /// Key of this query.
    type Key;

    /// Value of this query.
    type Value;

    /// True if salsa thinks that the value for `key` is a
    /// **constant**, meaning that it can never change, no matter what
    /// values the inputs take on from this point.
    fn is_constant(&self, key: Self::Key) -> bool;

    /// Get the (current) set of the entries in the query table.
    fn entries<C>(&self) -> C
    where
        C: FromIterator<TableEntry<Self::Key, Self::Value>>;
}

/// An entry from a query table, for debugging and inspecting the table state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableEntry<K, V> {
    /// key of the query
    pub key: K,
    /// value of the query, if it is stored
    pub value: Option<V>,
    _for_future_use: (),
}

impl<K, V> TableEntry<K, V> {
    pub(crate) fn new(key: K, value: Option<V>) -> TableEntry<K, V> {
        TableEntry {
            key,
            value,
            _for_future_use: (),
        }
    }
}

impl<DB, Q> DebugQueryTable for QueryTable<'_, DB, Q>
where
    DB: plumbing::GetQueryTable<Q>,
    Q: Query<DB>,
{
    type Key = Q::Key;
    type Value = Q::Value;

    fn is_constant(&self, key: Q::Key) -> bool {
        self.storage.is_constant(self.db, &key)
    }

    fn entries<C>(&self) -> C
    where
        C: FromIterator<TableEntry<Self::Key, Self::Value>>,
    {
        self.storage.entries(self.db)
    }
}
