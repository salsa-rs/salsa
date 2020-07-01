#![warn(rust_2018_idioms)]
#![warn(missing_docs)]

//! The salsa crate is a crate for incremental recomputation.  It
//! permits you to define a "database" of queries with both inputs and
//! values derived from those inputs; as you set the inputs, you can
//! re-execute the derived queries and it will try to re-use results
//! from previous invocations as appropriate.

mod dependency;
mod derived;
mod doctest;
mod durability;
mod input;
mod intern_id;
mod interned;
mod lru;
mod revision;
mod runtime;
mod blocking_future;

pub mod debug;
/// Items in this module are public for implementation reasons,
/// and are exempt from the SemVer guarantees.
#[doc(hidden)]
pub mod plumbing;

use crate::plumbing::DerivedQueryStorageOps;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::LruQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
use crate::revision::Revision;
use std::fmt::{self, Debug};
use std::hash::Hash;
use std::sync::Arc;

pub use crate::durability::Durability;
pub use crate::intern_id::InternId;
pub use crate::interned::InternKey;
pub use crate::runtime::Runtime;
pub use crate::runtime::RuntimeId;

/// The base trait which your "query context" must implement. Gives
/// access to the salsa runtime, which you must embed into your query
/// context (along with whatever other state you may require).
pub trait Database: plumbing::DatabaseStorageTypes + plumbing::DatabaseOps {
    /// Gives access to the underlying salsa runtime.
    fn salsa_runtime(&self) -> &Runtime<Self>;

    /// Gives access to the underlying salsa runtime.
    fn salsa_runtime_mut(&mut self) -> &mut Runtime<Self>;

    /// Iterates through all query storage and removes any values that
    /// have not been used since the last revision was created. The
    /// intended use-cycle is that you first execute all of your
    /// "main" queries; this will ensure that all query values they
    /// consume are marked as used.  You then invoke this method to
    /// remove other values that were not needed for your main query
    /// results.
    fn sweep_all(&self, strategy: SweepStrategy) {
        self.salsa_runtime().sweep_all(self, strategy);
    }

    /// Get access to extra methods pertaining to a given query. For
    /// example, you can use this to run the GC (`sweep`) across a
    /// single input. You can also use it to invoke a query, though
    /// it's more common to use the trait method on the database
    /// itself.
    #[allow(unused_variables)]
    fn query<Q>(&self, query: Q) -> QueryTable<'_, Self, Q>
    where
        Q: Query<Self>,
        Self: plumbing::GetQueryTable<Q>,
    {
        <Self as plumbing::GetQueryTable<Q>>::get_query_table(self)
    }

    /// Like `query`, but gives access to methods for setting the
    /// value of an input.
    ///
    /// # Threads, cancellation, and blocking
    ///
    /// Mutating the value of a query cannot be done while there are
    /// still other queries executing. If you are using your database
    /// within a single thread, this is not a problem: you only have
    /// `&self` access to the database, but this method requires `&mut
    /// self`.
    ///
    /// However, if you have used `snapshot` to create other threads,
    /// then attempts to `set` will **block the current thread** until
    /// those snapshots are dropped (usually when those threads
    /// complete). This also implies that if you create a snapshot but
    /// do not send it to another thread, then invoking `set` will
    /// deadlock.
    ///
    /// Before blocking, the thread that is attempting to `set` will
    /// also set a cancellation flag. In the threads operating on
    /// snapshots, you can use the [`is_current_revision_canceled`]
    /// method to check for this flag and bring those operations to a
    /// close, thus allowing the `set` to succeed. Ignoring this flag
    /// may lead to "starvation", meaning that the thread attempting
    /// to `set` has to wait a long, long time. =)
    ///
    /// [`is_current_revision_canceled`]: struct.Runtime.html#method.is_current_revision_canceled
    #[allow(unused_variables)]
    fn query_mut<Q>(&mut self, query: Q) -> QueryTableMut<'_, Self, Q>
    where
        Q: Query<Self>,
        Self: plumbing::GetQueryTable<Q>,
    {
        <Self as plumbing::GetQueryTable<Q>>::get_query_table_mut(self)
    }

    /// This function is invoked at key points in the salsa
    /// runtime. It permits the database to be customized and to
    /// inject logging or other custom behavior.
    fn salsa_event(&self, event_fn: impl Fn() -> Event<Self>) {
        #![allow(unused_variables)]
    }

    /// This function is invoked when a dependent query is being computed by the
    /// other thread, and that thread panics.
    fn on_propagated_panic(&self) -> ! {
        panic!("concurrent salsa query panicked")
    }
}

/// The `Event` struct identifies various notable things that can
/// occur during salsa execution. Instances of this struct are given
/// to `salsa_event`.
pub struct Event<DB: Database> {
    /// The id of the snapshot that triggered the event.  Usually
    /// 1-to-1 with a thread, as well.
    pub runtime_id: RuntimeId,

    /// What sort of event was it.
    pub kind: EventKind<DB>,
}

impl<DB: Database> fmt::Debug for Event<DB> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Event")
            .field("runtime_id", &self.runtime_id)
            .field("kind", &self.kind)
            .finish()
    }
}

/// An enum identifying the various kinds of events that can occur.
pub enum EventKind<DB: Database> {
    /// Occurs when we found that all inputs to a memoized value are
    /// up-to-date and hence the value can be re-used without
    /// executing the closure.
    ///
    /// Executes before the "re-used" value is returned.
    DidValidateMemoizedValue {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DB::DatabaseKey,
    },

    /// Indicates that another thread (with id `other_runtime_id`) is processing the
    /// given query (`database_key`), so we will block until they
    /// finish.
    ///
    /// Executes after we have registered with the other thread but
    /// before they have answered us.
    ///
    /// (NB: you can find the `id` of the current thread via the
    /// `salsa_runtime`)
    WillBlockOn {
        /// The id of the runtime we will block on.
        other_runtime_id: RuntimeId,

        /// The database-key for the affected value. Implements `Debug`.
        database_key: DB::DatabaseKey,
    },

    /// Indicates that the input value will change after this
    /// callback, e.g. due to a call to `set`.
    WillChangeInputValue {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DB::DatabaseKey,
    },

    /// Indicates that the function for this query will be executed.
    /// This is either because it has never executed before or because
    /// its inputs may be out of date.
    WillExecute {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DB::DatabaseKey,
    },
}

impl<DB: Database> fmt::Debug for EventKind<DB> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventKind::DidValidateMemoizedValue { database_key } => fmt
                .debug_struct("DidValidateMemoizedValue")
                .field("database_key", database_key)
                .finish(),
            EventKind::WillBlockOn {
                other_runtime_id,
                database_key,
            } => fmt
                .debug_struct("WillBlockOn")
                .field("other_runtime_id", other_runtime_id)
                .field("database_key", database_key)
                .finish(),
            EventKind::WillChangeInputValue { database_key } => fmt
                .debug_struct("WillChangeInputValue")
                .field("database_key", database_key)
                .finish(),
            EventKind::WillExecute { database_key } => fmt
                .debug_struct("WillExecute")
                .field("database_key", database_key)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DiscardIf {
    Never,
    Outdated,
    Always,
}

impl Default for DiscardIf {
    fn default() -> DiscardIf {
        DiscardIf::Never
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DiscardWhat {
    Nothing,
    Values,
    Everything,
}

impl Default for DiscardWhat {
    fn default() -> DiscardWhat {
        DiscardWhat::Nothing
    }
}

/// The sweep strategy controls what data we will keep/discard when we
/// do a GC-sweep. The default (`SweepStrategy::default`) is a no-op,
/// use `SweepStrategy::discard_outdated` constructor or `discard_*`
/// and `sweep_*` builder functions to construct useful strategies.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct SweepStrategy {
    discard_if: DiscardIf,
    discard_what: DiscardWhat,
}

impl SweepStrategy {
    /// Convenience function that discards all data not used thus far in the
    /// current revision.
    ///
    /// Equivalent to `SweepStrategy::default().discard_everything()`.
    pub fn discard_outdated() -> SweepStrategy {
        SweepStrategy::default()
            .discard_everything()
            .sweep_outdated()
    }

    /// Collects query values.
    ///
    /// Query dependencies are left in the database, which allows to quickly
    /// determine if the query is up to date, and avoid recomputing
    /// dependencies.
    pub fn discard_values(self) -> SweepStrategy {
        SweepStrategy {
            discard_what: self.discard_what.max(DiscardWhat::Values),
            ..self
        }
    }

    /// Collects both values and information about dependencies.
    ///
    /// Dependant queries will be recomputed even if all inputs to this query
    /// stay the same.
    pub fn discard_everything(self) -> SweepStrategy {
        SweepStrategy {
            discard_what: self.discard_what.max(DiscardWhat::Everything),
            ..self
        }
    }

    /// Process all keys, not verefied at the current revision.
    pub fn sweep_outdated(self) -> SweepStrategy {
        SweepStrategy {
            discard_if: self.discard_if.max(DiscardIf::Outdated),
            ..self
        }
    }

    /// Process all keys.
    pub fn sweep_all_revisions(self) -> SweepStrategy {
        SweepStrategy {
            discard_if: self.discard_if.max(DiscardIf::Always),
            ..self
        }
    }
}

/// Indicates a database that also supports parallel query
/// evaluation. All of Salsa's base query support is capable of
/// parallel execution, but for it to work, your query key/value types
/// must also be `Send`, as must any additional data in your database.
pub trait ParallelDatabase: Database + Send {
    /// Creates a second handle to the database that holds the
    /// database fixed at a particular revision. So long as this
    /// "frozen" handle exists, any attempt to [`set`] an input will
    /// block.
    ///
    /// [`set`]: struct.QueryTable.html#method.set
    ///
    /// This is the method you are meant to use most of the time in a
    /// parallel setting where modifications may arise asynchronously
    /// (e.g., a language server). In this context, it is common to
    /// wish to "fork off" a snapshot of the database performing some
    /// series of queries in parallel and arranging the results. Using
    /// this method for that purpose ensures that those queries will
    /// see a consistent view of the database (it is also advisable
    /// for those queries to use the [`is_current_revision_canceled`]
    /// method to check for cancellation).
    ///
    /// [`is_current_revision_canceled`]: struct.Runtime.html#method.is_current_revision_canceled
    ///
    /// # Panics
    ///
    /// It is not permitted to create a snapshot from inside of a
    /// query. Attepting to do so will panic.
    ///
    /// # Deadlock warning
    ///
    /// The intended pattern for snapshots is that, once created, they
    /// are sent to another thread and used from there. As such, the
    /// `snapshot` acquires a "read lock" on the database --
    /// therefore, so long as the `snapshot` is not dropped, any
    /// attempt to `set` a value in the database will block. If the
    /// `snapshot` is owned by the same thread that is attempting to
    /// `set`, this will cause a problem.
    ///
    /// # How to implement this
    ///
    /// Typically, this method will create a second copy of your
    /// database type (`MyDatabaseType`, in the example below),
    /// cloning over each of the fields from `self` into this new
    /// copy. For the field that stores the salsa runtime, you should
    /// use [the `Runtime::snapshot` method][rfm] to create a snapshot of the
    /// runtime. Finally, package up the result using `Snapshot::new`,
    /// which is a simple wrapper type that only gives `&self` access
    /// to the database within (thus preventing the use of methods
    /// that may mutate the inputs):
    ///
    /// [rfm]: struct.Runtime.html#method.snapshot
    ///
    /// ```rust,ignore
    /// impl ParallelDatabase for MyDatabaseType {
    ///     fn snapshot(&self) -> Snapshot<Self> {
    ///         Snapshot::new(
    ///             MyDatabaseType {
    ///                 runtime: self.runtime.snapshot(self),
    ///                 other_field: self.other_field.clone(),
    ///             }
    ///         )
    ///     }
    /// }
    /// ```
    fn snapshot(&self) -> Snapshot<Self>;
}

/// Simple wrapper struct that takes ownership of a database `DB` and
/// only gives `&self` access to it. See [the `snapshot` method][fm]
/// for more details.
///
/// [fm]: trait.ParallelDatabase.html#method.snapshot
#[derive(Debug)]
pub struct Snapshot<DB>
where
    DB: ParallelDatabase,
{
    db: DB,
}

impl<DB> Snapshot<DB>
where
    DB: ParallelDatabase,
{
    /// Creates a `Snapshot` that wraps the given database handle
    /// `db`. From this point forward, only shared references to `db`
    /// will be possible.
    pub fn new(db: DB) -> Self {
        Snapshot { db }
    }
}

impl<DB> std::ops::Deref for Snapshot<DB>
where
    DB: ParallelDatabase,
{
    type Target = DB;

    fn deref(&self) -> &DB {
        &self.db
    }
}

/// Trait implements by all of the "special types" associated with
/// each of your queries.
///
/// Unsafe trait obligation: Asserts that the Key/Value associated
/// types for this trait are a part of the `Group::GroupData` type.
/// In particular, `Group::GroupData: Send + Sync` must imply that
/// `Key: Send + Sync` and `Value: Send + Sync`. This is relied upon
/// by the dependency tracking logic.
pub unsafe trait Query<DB: Database>: Debug + Default + Sized + 'static {
    /// Type that you you give as a parameter -- for queries with zero
    /// or more than one input, this will be a tuple.
    type Key: Clone + Debug + Hash + Eq;

    /// What value does the query return?
    type Value: Clone + Debug;

    /// Internal struct storing the values for the query.
    type Storage: plumbing::QueryStorageOps<DB, Self>;

    /// Associate query group struct.
    type Group: plumbing::QueryGroup<
        DB,
        GroupStorage = Self::GroupStorage,
        GroupKey = Self::GroupKey,
    >;

    /// Generated struct that contains storage for all queries in a group.
    type GroupStorage;

    /// Type that identifies a particular query within the group + its key.
    type GroupKey;

    /// Extact storage for this query from the storage for its group.
    fn query_storage(group_storage: &Self::GroupStorage) -> &Arc<Self::Storage>;

    /// Create group key for this query.
    fn group_key(key: Self::Key) -> Self::GroupKey;
}

/// Return value from [the `query` method] on `Database`.
/// Gives access to various less common operations on queries.
///
/// [the `query` method]: trait.Database.html#method.query
pub struct QueryTable<'me, DB, Q>
where
    DB: plumbing::GetQueryTable<Q>,
    Q: Query<DB> + 'me,
{
    db: &'me DB,
    storage: &'me Q::Storage,
}

impl<'me, DB, Q> QueryTable<'me, DB, Q>
where
    DB: plumbing::GetQueryTable<Q>,
    Q: Query<DB>,
{
    /// Constructs a new `QueryTable`.
    pub fn new(db: &'me DB, storage: &'me Q::Storage) -> Self {
        Self { db, storage }
    }

    /// Execute the query on a given input. Usually it's easier to
    /// invoke the trait method directly. Note that for variadic
    /// queries (those with no inputs, or those with more than one
    /// input) the key will be a tuple.
    pub fn get(&self, key: Q::Key) -> Q::Value {
        self.try_get(key).unwrap_or_else(|err| panic!("{}", err))
    }

    fn try_get(&self, key: Q::Key) -> Result<Q::Value, CycleError<DB::DatabaseKey>> {
        self.storage.try_fetch(self.db, &key)
    }

    /// Remove all values for this query that have not been used in
    /// the most recent revision.
    pub fn sweep(&self, strategy: SweepStrategy)
    where
        Q::Storage: plumbing::QueryStorageMassOps<DB>,
    {
        self.storage.sweep(self.db, strategy);
    }
}

/// Return value from [the `query_mut` method] on `Database`.
/// Gives access to the `set` method, notably, that is used to
/// set the value of an input query.
///
/// [the `query_mut` method]: trait.Database.html#method.query_mut
pub struct QueryTableMut<'me, DB, Q>
where
    DB: plumbing::GetQueryTable<Q>,
    Q: Query<DB> + 'me,
{
    db: &'me mut DB,
    storage: Arc<Q::Storage>,
}

impl<'me, DB, Q> QueryTableMut<'me, DB, Q>
where
    DB: plumbing::GetQueryTable<Q>,
    Q: Query<DB>,
{
    /// Constructs a new `QueryTableMut`.
    pub fn new(db: &'me mut DB, storage: Arc<Q::Storage>) -> Self {
        Self { db, storage }
    }

    fn database_key(&self, key: &Q::Key) -> DB::DatabaseKey {
        <DB as plumbing::GetQueryTable<Q>>::database_key(&self.db, key.clone())
    }

    /// Assign a value to an "input query". Must be used outside of
    /// an active query computation.
    ///
    /// If you are using `snapshot`, see the notes on blocking
    /// and cancellation on [the `query_mut` method].
    ///
    /// [the `query_mut` method]: trait.Database.html#method.query_mut
    pub fn set(&mut self, key: Q::Key, value: Q::Value)
    where
        Q::Storage: plumbing::InputQueryStorageOps<DB, Q>,
    {
        self.set_with_durability(key, value, Durability::LOW);
    }

    /// Assign a value to an "input query", with the additional
    /// promise that this value will **never change**. Must be used
    /// outside of an active query computation.
    ///
    /// If you are using `snapshot`, see the notes on blocking
    /// and cancellation on [the `query_mut` method].
    ///
    /// [the `query_mut` method]: trait.Database.html#method.query_mut
    pub fn set_with_durability(&mut self, key: Q::Key, value: Q::Value, durability: Durability)
    where
        Q::Storage: plumbing::InputQueryStorageOps<DB, Q>,
    {
        self.storage
            .set(self.db, &key, &self.database_key(&key), value, durability);
    }

    /// Sets the size of LRU cache of values for this query table.
    ///
    /// That is, at most `cap` values will be preset in the table at the same
    /// time. This helps with keeping maximum memory usage under control, at the
    /// cost of potential extra recalculations of evicted values.
    ///
    /// If `cap` is zero, all values are preserved, this is the default.
    pub fn set_lru_capacity(&self, cap: usize)
    where
        Q::Storage: plumbing::LruQueryStorageOps,
    {
        self.storage.set_lru_capacity(cap);
    }

    /// Marks the computed value as outdated.
    ///
    /// This causes salsa to re-execute the query function on the next access to
    /// the query, even if all dependencies are up to date.
    ///
    /// This is most commonly used as part of the [on-demand input
    /// pattern](https://salsa-rs.github.io/salsa/common_patterns/on_demand_inputs.html).
    pub fn invalidate(&mut self, key: &Q::Key)
    where
        Q::Storage: plumbing::DerivedQueryStorageOps<DB, Q>,
    {
        self.storage.invalidate(self.db, key)
    }
}

/// The error returned when a query could not be resolved due to a cycle
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct CycleError<K> {
    /// The queries that were part of the cycle
    cycle: Vec<K>,
    changed_at: Revision,
    durability: Durability,
}

impl<K> fmt::Display for CycleError<K>
where
    K: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Internal error, cycle detected:\n")?;
        for i in &self.cycle {
            writeln!(f, "{:?}", i)?;
        }
        Ok(())
    }
}

// Re-export the procedural macros.
#[allow(unused_imports)]
#[macro_use]
extern crate salsa_macros;
#[doc(hidden)]
pub use salsa_macros::*;
