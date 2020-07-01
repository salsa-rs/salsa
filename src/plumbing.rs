#![allow(missing_docs)]

use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::CycleError;
use crate::Database;
use crate::Query;
use crate::QueryTable;
use crate::QueryTableMut;
use crate::RuntimeId;
use crate::SweepStrategy;
use std::fmt::Debug;
use std::hash::Hash;

pub use crate::derived::DependencyStorage;
pub use crate::derived::MemoizedStorage;
pub use crate::input::InputStorage;
pub use crate::interned::InternedStorage;
pub use crate::interned::LookupInternedStorage;
pub use crate::{revision::Revision, DatabaseKeyIndex};

#[derive(Clone, Debug)]
pub struct CycleDetected {
    pub(crate) from: RuntimeId,
    pub(crate) to: RuntimeId,
}

/// Defines various associated types. An impl of this
/// should be generated for your query-context type automatically by
/// the `database_storage` macro, so you shouldn't need to mess
/// with this trait directly.
pub trait DatabaseStorageTypes: Sized {
    /// A "query descriptor" packages up all the possible queries and a key.
    /// It is used to store information about (e.g.) the stack.
    ///
    /// At runtime, it can be implemented in various ways: a monster enum
    /// works for a fixed set of queries, but a boxed trait object is good
    /// for a more open-ended option.
    type DatabaseKey: DatabaseKey<Self>;

    /// Defines the "storage type", where all the query data is kept.
    /// This type is defined by the `database_storage` macro.
    type DatabaseStorage: Default;
}

/// Internal operations that the runtime uses to operate on the database.
pub trait DatabaseOps: Sized {
    /// Formats a database key index in a human readable fashion.
    fn fmt_index(
        &self,
        index: DatabaseKeyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result;

    /// True if the computed value for `input` may have changed since `revision`.
    fn maybe_changed_since(&self, input: DatabaseKeyIndex, revision: Revision) -> bool;

    /// Executes the callback for each kind of query.
    fn for_each_query(&self, op: impl FnMut(&dyn QueryStorageMassOps<Self>));
}

/// Internal operations performed on the query storage as a whole
/// (note that these ops do not need to know the identity of the
/// query, unlike `QueryStorageOps`).
pub trait QueryStorageMassOps<DB: Database> {
    /// Discards memoized values that are not up to date with the current revision.
    fn sweep(&self, db: &DB, strategy: SweepStrategy);
}

pub trait DatabaseKey<DB>: Clone + Debug + Eq + Hash {}

pub trait QueryFunction<DB: Database>: Query<DB> {
    fn execute(db: &DB, key: Self::Key) -> Self::Value;
    fn recover(db: &DB, cycle: &[DatabaseKeyIndex], key: &Self::Key) -> Option<Self::Value> {
        let _ = (db, cycle, key);
        None
    }
}

/// The `GetQueryTable` trait makes the connection the *database type*
/// `DB` and some specific *query type* `Q` that it supports. Note
/// that the `Database` trait itself is not specific to any query, and
/// the impls of the query trait are not specific to any *database*
/// (in particular, query groups are defined without knowing the final
/// database type). This trait then serves to put the query in the
/// context of the full database. It gives access to the storage for
/// the query and also to creating the query descriptor. For any given
/// database, impls of this trait are created by the
/// `database_storage` macro.
pub trait GetQueryTable<Q: Query<Self>>: Database {
    /// Create a query table, which has access to the storage for the query
    /// and offers methods like `get`.
    fn get_query_table(db: &Self) -> QueryTable<'_, Self, Q>;

    /// Create a mutable query table, which has access to the storage
    /// for the query and offers methods like `set`.
    fn get_query_table_mut(db: &mut Self) -> QueryTableMut<'_, Self, Q>;

    /// Create a query descriptor given a key for this query.
    fn database_key(db: &Self, key: Q::Key) -> Self::DatabaseKey;
}

impl<DB, Q> GetQueryTable<Q> for DB
where
    DB: Database,
    Q: Query<DB>,
    DB: HasQueryGroup<Q::Group>,
{
    fn get_query_table(db: &DB) -> QueryTable<'_, DB, Q> {
        let group_storage: &Q::GroupStorage = HasQueryGroup::group_storage(db);
        let query_storage: &Q::Storage = Q::query_storage(group_storage);
        QueryTable::new(db, query_storage)
    }

    fn get_query_table_mut(db: &mut DB) -> QueryTableMut<'_, DB, Q> {
        let group_storage: &Q::GroupStorage = HasQueryGroup::group_storage(db);
        let query_storage = Q::query_storage(group_storage).clone();
        QueryTableMut::new(db, query_storage)
    }

    fn database_key(
        _db: &DB,
        key: <Q as Query<DB>>::Key,
    ) -> <DB as DatabaseStorageTypes>::DatabaseKey {
        let group_key = Q::group_key(key);
        <DB as HasQueryGroup<_>>::database_key(group_key)
    }
}

pub trait QueryGroup<DB: Database> {
    type GroupStorage;
    type GroupKey;
}

/// Trait implemented by a database for each group that it supports.
/// `S` and `K` are the types for *group storage* and *group key*, respectively.
pub trait HasQueryGroup<G>: Database
where
    G: QueryGroup<Self>,
{
    /// Access the group storage struct from the database.
    fn group_storage(db: &Self) -> &G::GroupStorage;

    /// "Upcast" a group key into a database key.
    fn database_key(group_key: G::GroupKey) -> Self::DatabaseKey;
}

pub trait QueryStorageOps<DB, Q>
where
    Self: QueryStorageMassOps<DB>,
    DB: Database,
    Q: Query<DB>,
{
    fn new(group_index: u16) -> Self;

    /// Format a database key index in a suitable way.
    fn fmt_index(
        &self,
        db: &DB,
        index: DatabaseKeyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result;

    /// True if the value of `input`, which must be from this query, may have
    /// changed since the given revision.
    fn maybe_changed_since(&self, db: &DB, input: DatabaseKeyIndex, revision: Revision) -> bool;

    /// Execute the query, returning the result (often, the result
    /// will be memoized).  This is the "main method" for
    /// queries.
    ///
    /// Returns `Err` in the event of a cycle, meaning that computing
    /// the value for this `key` is recursively attempting to fetch
    /// itself.
    fn try_fetch(&self, db: &DB, key: &Q::Key) -> Result<Q::Value, CycleError<DatabaseKeyIndex>>;

    /// Returns the durability associated with a given key.
    fn durability(&self, db: &DB, key: &Q::Key) -> Durability;

    /// Get the (current) set of the entries in the query storage
    fn entries<C>(&self, db: &DB) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>;
}

/// An optional trait that is implemented for "user mutable" storage:
/// that is, storage whose value is not derived from other storage but
/// is set independently.
pub trait InputQueryStorageOps<DB, Q>
where
    DB: Database,
    Q: Query<DB>,
{
    fn set(&self, db: &mut DB, key: &Q::Key, new_value: Q::Value, durability: Durability);
}

/// An optional trait that is implemented for "user mutable" storage:
/// that is, storage whose value is not derived from other storage but
/// is set independently.
pub trait LruQueryStorageOps {
    fn set_lru_capacity(&self, new_capacity: usize);
}

pub trait DerivedQueryStorageOps<DB, Q>
where
    DB: Database,
    Q: Query<DB>,
{
    fn invalidate(&self, db: &mut DB, key: &Q::Key);
}
