use crate::Database;
use crate::Query;
use crate::QueryTable;
use crate::SweepStrategy;
use std::fmt::Debug;
use std::hash::Hash;

pub use crate::derived::DependencyStorage;
pub use crate::derived::MemoizedStorage;
pub use crate::derived::VolatileStorage;
pub use crate::input::DefaultEqValueInputPolicy;
pub use crate::input::DefaultValueInputPolicy;
pub use crate::input::EqValueInputPolicy;
pub use crate::input::ExplicitInputPolicy;
pub use crate::input::InputStorage;
pub use crate::runtime::Revision;

pub struct CycleDetected;

/// Defines the `QueryDescriptor` associated type. An impl of this
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
    type QueryDescriptor: QueryDescriptor<Self>;

    /// Defines the "storage type", where all the query data is kept.
    /// This type is defined by the `database_storage` macro.
    type DatabaseStorage: Default;
}

/// Internal operations that the runtime uses to operate on the database.
pub trait DatabaseOps: Sized {
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

pub trait QueryDescriptor<DB>: Clone + Debug + Eq + Hash + Send + Sync {
    /// Returns true if the value of this query may have changed since
    /// the given revision.
    fn maybe_changed_since(&self, db: &DB, revision: Revision) -> bool;
}

pub trait QueryFunction<DB: Database>: Query<DB> {
    fn execute(db: &DB, key: Self::Key) -> Self::Value;
}

pub trait GetQueryTable<Q: Query<Self>>: Database {
    fn get_query_table(db: &Self) -> QueryTable<'_, Self, Q>;
}

pub trait QueryStorageOps<DB, Q>: Default
where
    DB: Database,
    Q: Query<DB>,
{
    /// Execute the query, returning the result (often, the result
    /// will be memoized).  This is the "main method" for
    /// queries.
    ///
    /// Returns `Err` in the event of a cycle, meaning that computing
    /// the value for this `key` is recursively attempting to fetch
    /// itself.
    fn try_fetch(
        &self,
        db: &DB,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> Result<Q::Value, CycleDetected>;

    /// True if the query **may** have changed since the given
    /// revision. The query will answer this question with as much
    /// precision as it is able to do based on its storage type.  In
    /// the event of a cycle being detected as part of this function,
    /// it returns true.
    ///
    /// Example: The steps for a memoized query are as follows.
    ///
    /// - If the query has already been computed:
    ///   - Check the inputs that the previous computation used
    ///     recursively to see if *they* have changed.  If they have
    ///     not, then return false.
    ///   - If they have, then the query is re-executed and the new
    ///     result is compared against the old result. If it is equal,
    ///     then return false.
    /// - Return true.
    ///
    /// Other storage types will skip some or all of these steps.
    fn maybe_changed_since(
        &self,
        db: &DB,
        revision: Revision,
        key: &Q::Key,
        descriptor: &DB::QueryDescriptor,
    ) -> bool;

    /// Check if `key` is (currently) believed to be a constant.
    fn is_constant(&self, db: &DB, key: &Q::Key) -> bool;

    /// Check if `key` is (currently) believed to be a constant.
    fn keys<C>(&self, db: &DB) -> C
    where
        C: std::iter::FromIterator<Q::Key>;
}

/// An optional trait that is implemented for "user mutable" storage:
/// that is, storage whose value is not derived from other storage but
/// is set independently.
pub trait InputQueryStorageOps<DB, Q>: Default
where
    DB: Database,
    Q: Query<DB>,
{
    fn set(&self, db: &DB, key: &Q::Key, new_value: Q::Value);

    fn set_constant(&self, db: &DB, key: &Q::Key, new_value: Q::Value);
}

/// An optional trait that is implemented for "user mutable" storage:
/// that is, storage whose value is not derived from other storage but
/// is set independently.
pub trait UncheckedMutQueryStorageOps<DB, Q>: Default
where
    DB: Database,
    Q: Query<DB>,
{
    fn set_unchecked(&self, db: &DB, key: &Q::Key, new_value: Q::Value);
}
