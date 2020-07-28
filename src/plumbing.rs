#![allow(missing_docs)]

use crate::debug::TableEntry;
use crate::durability::Durability;
use crate::AsAsyncDatabase;
use crate::CycleError;
use crate::Database;
use crate::Query;
use crate::QueryTable;
use crate::QueryTableMut;
use crate::RuntimeId;
use crate::SweepStrategy;
use std::fmt::Debug;
use std::{
    future::Future,
    hash::Hash,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[cfg(feature = "async")]
pub use crate::blocking_future::BlockingAsyncFuture;
pub use crate::derived::DependencyStorage;
pub use crate::derived::{MemoizedStorage, WaitResult};
pub use crate::input::InputStorage;
pub use crate::interned::InternedStorage;
pub use crate::interned::LookupInternedStorage;
pub use crate::{
    blocking_future::{BlockingFuture, BlockingFutureTrait},
    revision::Revision,
    BoxFuture, DatabaseKeyIndex, QueryBase, QueryDb, Runtime,
};

#[derive(Clone, Debug)]
pub struct CycleDetected {
    pub(crate) from: RuntimeId,
    pub(crate) to: RuntimeId,
}

/// Defines various associated types. An impl of this
/// should be generated for your query-context type automatically by
/// the `database_storage` macro, so you shouldn't need to mess
/// with this trait directly.
pub trait DatabaseStorageTypes: Database {
    /// Defines the "storage type", where all the query data is kept.
    /// This type is defined by the `database_storage` macro.
    type DatabaseStorage: Default;
}

/// Internal operations that the runtime uses to operate on the database.
pub trait DatabaseOps {
    /// Upcast this type to a `dyn Database`.
    fn ops_database(&self) -> &dyn Database;

    /// Gives access to the underlying salsa runtime.
    fn ops_salsa_runtime(&self) -> &Runtime;

    /// Gives access to the underlying salsa runtime.
    fn ops_salsa_runtime_mut(&mut self) -> &mut Runtime;

    /// Formats a database key index in a human readable fashion.
    fn fmt_index(
        &self,
        index: DatabaseKeyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result;

    /// True if the computed value for `input` may have changed since `revision`.
    fn maybe_changed_since(&self, input: DatabaseKeyIndex, revision: Revision) -> bool;

    /// True if the computed value for `input` may have changed since `revision`.
    fn maybe_changed_since_async(
        &mut self,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> BoxFuture<'_, bool>;

    /// Executes the callback for each kind of query.
    fn for_each_query(&self, op: &mut dyn FnMut(&dyn QueryStorageMassOps));
}

/// Internal operations performed on the query storage as a whole
/// (note that these ops do not need to know the identity of the
/// query, unlike `QueryStorageOps`).
pub trait QueryStorageMassOps {
    /// Discards memoized values that are not up to date with the current revision.
    fn sweep(&self, runtime: &Runtime, strategy: SweepStrategy);
    fn purge(&self);
}

pub trait DatabaseKey: Clone + Debug + Eq + Hash {}

pub trait QueryFunctionBase: QueryBase {
    type BlockingFuture: BlockingFutureTrait<WaitResult<Self::Value, DatabaseKeyIndex>>;
}

pub trait QueryFunction<'f, 'd>: QueryFunctionBase + QueryDb<'d> {
    type Future: Future<Output = Self::Value> + 'f;

    fn execute(db: &'f mut <Self as QueryDb<'d>>::Db, key: Self::Key) -> Self::Future;

    fn recover(
        db: &<Self as QueryDb<'d>>::DynDb,
        cycle: &[DatabaseKeyIndex],
        key: &Self::Key,
    ) -> Option<Self::Value> {
        let _ = (db, cycle, key);
        None
    }
}

// Workaround for `for<'d> <Q as QueryDb<'d>>::Db: Send` being impossible to fulfill at callsites (in the generated code).
// Helps rustc understand that the future and database are actually `Send`
#[doc(hidden)]
#[cfg(feature = "async")]
pub trait AsyncQueryFunction<'f, 'd>:
    QueryFunction<
    'f,
    'd,
    DynDb = <Self as AsyncQueryFunction<'f, 'd>>::SendDynDb,
    Db = <Self as AsyncQueryFunction<'f, 'd>>::SendDb,
    Future = crate::BoxFuture<'f, <Self as QueryBase>::Value>,
>
where
    <Self as QueryBase>::Value: Send + 'f,
{
    type SendDynDb: ?Sized + Database + HasQueryGroup<Self::Group> + Send + 'd;
    type SendDb: std::ops::Deref<Target = Self::DynDb>
        + AsAsyncDatabase<Self::SendDynDb>
        + Send
        + 'd;
}

/// Create a query table, which has access to the storage for the query
/// and offers methods like `get`.
pub fn get_query_table<'me, Q>(
    db: &'me <Q as QueryDb<'me>>::DynDb,
) -> QueryTable<'me, Q, &'me <Q as QueryDb<'me>>::DynDb>
where
    Q: Query,
    Q::Storage: QueryStorageOps<Q>,
{
    let group_storage: &Q::GroupStorage = HasQueryGroup::group_storage(&*db);
    let query_storage: Arc<Q::Storage> = Q::query_storage(group_storage).clone();
    QueryTable::new(db, query_storage)
}

pub fn get_query_table_async<'me, Q>(
    db: <Q as QueryDb<'me>>::Db,
) -> QueryTable<'me, Q, <Q as QueryDb<'me>>::Db>
where
    Q: Query,
    Q::Storage: QueryStorageOps<Q>,
{
    let group_storage: &Q::GroupStorage = HasQueryGroup::group_storage(&*db);
    let query_storage: Arc<Q::Storage> = Q::query_storage(group_storage).clone();
    QueryTable::new_async(db, query_storage)
}

/// Create a mutable query table, which has access to the storage
/// for the query and offers methods like `set`.
pub fn get_query_table_mut<'me, Q>(db: &'me mut <Q as QueryDb<'me>>::DynDb) -> QueryTableMut<'me, Q>
where
    Q: Query,
{
    let group_storage: &Q::GroupStorage = HasQueryGroup::group_storage(db);
    let query_storage = Q::query_storage(group_storage).clone();
    QueryTableMut::new(db, query_storage)
}

pub trait QueryGroup: Sized {
    type GroupStorage;

    /// Dyn version of the associated database trait.
    type DynDb: ?Sized + Database + HasQueryGroup<Self>;
}

/// Trait implemented by a database for each group that it supports.
/// `S` and `K` are the types for *group storage* and *group key*, respectively.
pub trait HasQueryGroup<G>: Database
where
    G: QueryGroup,
{
    /// Access the group storage struct from the database.
    fn group_storage(&self) -> &G::GroupStorage;
}

pub trait QueryStorageOps<Q>
where
    Self: QueryStorageMassOps,
    Q: Query,
{
    fn new(group_index: u16) -> Self;

    /// Format a database key index in a suitable way.
    fn fmt_index(
        &self,
        db: &<Q as QueryDb<'_>>::DynDb,
        index: DatabaseKeyIndex,
        fmt: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result;

    /// Returns the durability associated with a given key.
    fn durability(&self, db: &<Q as QueryDb<'_>>::DynDb, key: &Q::Key) -> Durability;

    /// Get the (current) set of the entries in the query storage
    fn entries<C>(&self, db: &<Q as QueryDb<'_>>::DynDb) -> C
    where
        C: std::iter::FromIterator<TableEntry<Q::Key, Q::Value>>;

    fn peek(&self, db: &<Q as QueryDb<'_>>::DynDb, _key: &Q::Key) -> Option<Q::Value>;
}

pub trait QueryStorageOpsSync<Q>: QueryStorageOps<Q>
where
    Self: QueryStorageMassOps,
    Q: Query,
{
    /// True if the value of `input`, which must be from this query, may have
    /// changed since the given revision.
    fn maybe_changed_since(
        &self,
        db: &mut <Q as QueryDb<'_>>::Db,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> bool;

    /// Execute the query, returning the result (often, the result
    /// will be memoized).  This is the "main method" for
    /// queries.
    ///
    /// Returns `Err` in the event of a cycle, meaning that computing
    /// the value for this `key` is recursively attempting to fetch
    /// itself.
    fn try_fetch(
        &self,
        db: &mut <Q as QueryDb<'_>>::Db,
        key: &Q::Key,
    ) -> Result<Q::Value, CycleError<DatabaseKeyIndex>>;
}

#[cfg(feature = "async")]
pub trait QueryStorageOpsAsync<Q>: QueryStorageOps<Q>
where
    Self: QueryStorageMassOps,
    Q: for<'f, 'd> AsyncQueryFunction<'f, 'd>,
    Q::Key: Send + Sync,
    Q::Value: Send + Sync,
{
    /// True if the value of `input`, which must be from this query, may have
    /// changed since the given revision.
    fn maybe_changed_since_async<'f>(
        &'f self,
        db: &'f mut <Q as AsyncQueryFunction<'_, '_>>::SendDb,
        input: DatabaseKeyIndex,
        revision: Revision,
    ) -> crate::BoxFuture<'f, bool>;

    /// Execute the query, returning the result (often, the result
    /// will be memoized).  This is the "main method" for
    /// queries.
    ///
    /// Returns `Err` in the event of a cycle, meaning that computing
    /// the value for this `key` is recursively attempting to fetch
    /// itself.
    fn try_fetch_async<'f>(
        &'f self,
        db: &'f mut <Q as AsyncQueryFunction<'_, '_>>::SendDb,
        key: &'f Q::Key,
    ) -> crate::BoxFuture<'f, Result<Q::Value, CycleError<DatabaseKeyIndex>>>;
}

/// An optional trait that is implemented for "user mutable" storage:
/// that is, storage whose value is not derived from other storage but
/// is set independently.
pub trait InputQueryStorageOps<Q>
where
    Q: Query,
{
    fn set(
        &self,
        db: &mut <Q as QueryDb<'_>>::DynDb,
        key: &Q::Key,
        new_value: Q::Value,
        durability: Durability,
    );
}

/// An optional trait that is implemented for "user mutable" storage:
/// that is, storage whose value is not derived from other storage but
/// is set independently.
pub trait LruQueryStorageOps {
    fn set_lru_capacity(&self, new_capacity: usize);
}

pub trait DerivedQueryStorageOps<Q>
where
    Q: Query,
{
    fn invalidate(&self, db: &mut <Q as QueryDb<'_>>::DynDb, key: &Q::Key);
}

/// Calls a future synchronously without an actual way to resume to future.
pub(crate) fn sync_future<F>(mut f: F) -> F::Output
where
    F: Future,
{
    use std::task::{RawWaker, RawWakerVTable, Waker};

    unsafe {
        type WakerState = ();

        static VTABLE: RawWakerVTable =
            RawWakerVTable::new(|p| RawWaker::new(p, &VTABLE), |_| (), |_| (), |_| ());

        let waker_state = WakerState::default();
        let waker = Waker::from_raw(RawWaker::new(
            &waker_state as *const WakerState as *const (),
            &VTABLE,
        ));
        let mut context = Context::from_waker(&waker);

        match Pin::new_unchecked(&mut f).poll(&mut context) {
            Poll::Ready(x) => x,
            Poll::Pending => unreachable!(),
        }
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Ready<T>(Option<T>);

impl<T> Unpin for Ready<T> {}

impl<T> Future for Ready<T> {
    type Output = T;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
        Poll::Ready(self.0.take().expect("Ready polled after completion"))
    }
}

pub fn ready<T>(t: T) -> Ready<T> {
    Ready(Some(t))
}
