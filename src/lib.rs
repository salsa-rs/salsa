#![warn(rust_2018_idioms)]
#![warn(missing_docs)]

//! The salsa crate is a crate for incremental recomputation.  It
//! permits you to define a "database" of queries with both inputs and
//! values derived from those inputs; as you set the inputs, you can
//! re-execute the derived queries and it will try to re-use results
//! from previous invocations as appropriate.

mod blocking_future;
mod derived;
mod doctest;
mod durability;
mod input;
mod intern_id;
mod interned;
mod lru;
mod revision;
mod runtime;
mod storage;

pub mod debug;
/// Items in this module are public for implementation reasons,
/// and are exempt from the SemVer guarantees.
#[doc(hidden)]
pub mod plumbing;

use crate::plumbing::DatabaseOps;
use crate::plumbing::DerivedQueryStorageOps;
use crate::plumbing::InputQueryStorageOps;
use crate::plumbing::LruQueryStorageOps;
use crate::plumbing::QueryStorageMassOps;
use crate::plumbing::QueryStorageOps;
pub use crate::revision::Revision;
use std::fmt::{self, Debug};
use std::hash::Hash;
use std::panic::{self, UnwindSafe};
use std::sync::Arc;

pub use crate::durability::Durability;
pub use crate::intern_id::InternId;
pub use crate::interned::InternKey;
pub use crate::runtime::Runtime;
pub use crate::runtime::RuntimeId;
pub use crate::storage::Storage;

/// The base trait which your "query context" must implement. Gives
/// access to the salsa runtime, which you must embed into your query
/// context (along with whatever other state you may require).
pub trait Database: plumbing::DatabaseOps {
    /// Iterates through all query storage and removes any values that
    /// have not been used since the last revision was created. The
    /// intended use-cycle is that you first execute all of your
    /// "main" queries; this will ensure that all query values they
    /// consume are marked as used.  You then invoke this method to
    /// remove other values that were not needed for your main query
    /// results.
    ///
    /// This method should not be overridden by `Database` implementors.
    fn sweep_all(&self, strategy: SweepStrategy) {
        // Note that we do not acquire the query lock (or any locks)
        // here.  Each table is capable of sweeping itself atomically
        // and there is no need to bring things to a halt. That said,
        // users may wish to guarantee atomicity.

        let runtime = self.salsa_runtime();
        self.for_each_query(&mut |query_storage| query_storage.sweep(runtime, strategy));
    }

    /// This function is invoked at key points in the salsa
    /// runtime. It permits the database to be customized and to
    /// inject logging or other custom behavior.
    fn salsa_event(&self, event_fn: Event) {
        #![allow(unused_variables)]
    }

    /// Starts unwinding the stack if the current revision is cancelled.
    ///
    /// This method can be called by query implementations that perform
    /// potentially expensive computations, in order to speed up propagation of
    /// cancellation.
    ///
    /// Cancellation will automatically be triggered by salsa on any query
    /// invocation.
    ///
    /// This method should not be overridden by `Database` implementors. A
    /// `salsa_event` is emitted when this method is called, so that should be
    /// used instead.
    #[inline]
    fn unwind_if_cancelled(&self) {
        let runtime = self.salsa_runtime();
        self.salsa_event(Event {
            runtime_id: runtime.id(),
            kind: EventKind::WillCheckCancellation,
        });

        let current_revision = runtime.current_revision();
        let pending_revision = runtime.pending_revision();
        log::debug!(
            "unwind_if_cancelled: current_revision={:?}, pending_revision={:?}",
            current_revision,
            pending_revision
        );
        if pending_revision > current_revision {
            runtime.unwind_cancelled();
        }
    }

    /// Gives access to the underlying salsa runtime.
    ///
    /// This method should not be overridden by `Database` implementors.
    fn salsa_runtime(&self) -> &Runtime {
        self.ops_salsa_runtime()
    }

    /// Gives access to the underlying salsa runtime.
    ///
    /// This method should not be overridden by `Database` implementors.
    fn salsa_runtime_mut(&mut self) -> &mut Runtime {
        self.ops_salsa_runtime_mut()
    }
}

/// The `Event` struct identifies various notable things that can
/// occur during salsa execution. Instances of this struct are given
/// to `salsa_event`.
pub struct Event {
    /// The id of the snapshot that triggered the event.  Usually
    /// 1-to-1 with a thread, as well.
    pub runtime_id: RuntimeId,

    /// What sort of event was it.
    pub kind: EventKind,
}

impl fmt::Debug for Event {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Event")
            .field("runtime_id", &self.runtime_id)
            .field("kind", &self.kind)
            .finish()
    }
}

/// An enum identifying the various kinds of events that can occur.
pub enum EventKind {
    /// Occurs when we found that all inputs to a memoized value are
    /// up-to-date and hence the value can be re-used without
    /// executing the closure.
    ///
    /// Executes before the "re-used" value is returned.
    DidValidateMemoizedValue {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DatabaseKeyIndex,
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
        database_key: DatabaseKeyIndex,
    },

    /// Indicates that the function for this query will be executed.
    /// This is either because it has never executed before or because
    /// its inputs may be out of date.
    WillExecute {
        /// The database-key for the affected value. Implements `Debug`.
        database_key: DatabaseKeyIndex,
    },

    /// Indicates that `unwind_if_cancelled` was called and salsa will check if
    /// the current revision has been cancelled.
    WillCheckCancellation,
}

impl fmt::Debug for EventKind {
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
            EventKind::WillExecute { database_key } => fmt
                .debug_struct("WillExecute")
                .field("database_key", database_key)
                .finish(),
            EventKind::WillCheckCancellation => fmt.debug_struct("WillCheckCancellation").finish(),
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
    shrink_to_fit: bool,
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
    /// for those queries to use the [`Runtime::unwind_if_cancelled`]
    /// method to check for cancellation).
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
pub struct Snapshot<DB: ?Sized>
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

/// An integer that uniquely identifies a particular query instance within the
/// database. Used to track dependencies between queries. Fully ordered and
/// equatable but those orderings are arbitrary, and meant to be used only for
/// inserting into maps and the like.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DatabaseKeyIndex {
    group_index: u16,
    query_index: u16,
    key_index: u32,
}

impl DatabaseKeyIndex {
    /// Returns the index of the query group containing this key.
    #[inline]
    pub fn group_index(self) -> u16 {
        self.group_index
    }

    /// Returns the index of the query within its query group.
    #[inline]
    pub fn query_index(self) -> u16 {
        self.query_index
    }

    /// Returns the index of this particular query key within the query.
    #[inline]
    pub fn key_index(self) -> u32 {
        self.key_index
    }

    /// Returns a type that gives a user-readable debug output.
    /// Use like `println!("{:?}", index.debug(db))`.
    pub fn debug<D: ?Sized>(self, db: &D) -> impl std::fmt::Debug + '_
    where
        D: plumbing::DatabaseOps,
    {
        DatabaseKeyIndexDebug { index: self, db }
    }
}

/// Helper type for `DatabaseKeyIndex::debug`
struct DatabaseKeyIndexDebug<'me, D: ?Sized>
where
    D: plumbing::DatabaseOps,
{
    index: DatabaseKeyIndex,
    db: &'me D,
}

impl<D: ?Sized> std::fmt::Debug for DatabaseKeyIndexDebug<'_, D>
where
    D: plumbing::DatabaseOps,
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.db.fmt_index(self.index, fmt)
    }
}

/// Trait implements by all of the "special types" associated with
/// each of your queries.
///
/// Base trait of `Query` that has a lifetime parameter to allow the `DynDb` to be non-'static.
pub trait QueryDb<'d>: Sized {
    /// Dyn version of the associated trait for this query group.
    type DynDb: ?Sized + Database + HasQueryGroup<Self::Group> + 'd;

    /// Associate query group struct.
    type Group: plumbing::QueryGroup<GroupStorage = Self::GroupStorage>;

    /// Generated struct that contains storage for all queries in a group.
    type GroupStorage;
}

/// Trait implements by all of the "special types" associated with
/// each of your queries.
pub trait Query: Debug + Default + Sized + for<'d> QueryDb<'d> {
    /// Type that you you give as a parameter -- for queries with zero
    /// or more than one input, this will be a tuple.
    type Key: Clone + Debug + Hash + Eq;

    /// What value does the query return?
    type Value: Clone + Debug;

    /// Internal struct storing the values for the query.
    // type Storage: plumbing::QueryStorageOps<Self>;
    type Storage;

    /// A unique index identifying this query within the group.
    const QUERY_INDEX: u16;

    /// Name of the query method (e.g., `foo`)
    const QUERY_NAME: &'static str;

    /// Extact storage for this query from the storage for its group.
    fn query_storage<'a>(
        group_storage: &'a <Self as QueryDb<'_>>::GroupStorage,
    ) -> &'a Arc<Self::Storage>;
}

/// Return value from [the `query` method] on `Database`.
/// Gives access to various less common operations on queries.
///
/// [the `query` method]: trait.Database.html#method.query
pub struct QueryTable<'me, Q>
where
    Q: Query,
{
    db: &'me <Q as QueryDb<'me>>::DynDb,
    storage: &'me Q::Storage,
}

impl<'me, Q> QueryTable<'me, Q>
where
    Q: Query,
    Q::Storage: QueryStorageOps<Q>,
{
    /// Constructs a new `QueryTable`.
    pub fn new(db: &'me <Q as QueryDb<'me>>::DynDb, storage: &'me Q::Storage) -> Self {
        Self { db, storage }
    }

    /// Execute the query on a given input. Usually it's easier to
    /// invoke the trait method directly. Note that for variadic
    /// queries (those with no inputs, or those with more than one
    /// input) the key will be a tuple.
    pub fn get(&self, key: Q::Key) -> Q::Value {
        self.try_get(key)
            .unwrap_or_else(|err| panic!("{:?}", err.debug(self.db)))
    }

    fn try_get(&self, key: Q::Key) -> Result<Q::Value, CycleError<DatabaseKeyIndex>> {
        self.storage.try_fetch(self.db, &key)
    }

    /// Remove all values for this query that have not been used in
    /// the most recent revision.
    pub fn sweep(&self, strategy: SweepStrategy)
    where
        Q::Storage: plumbing::QueryStorageMassOps,
    {
        self.storage.sweep(self.db.salsa_runtime(), strategy);
    }
    /// Completely clears the storage for this query.
    ///
    /// This method breaks internal invariants of salsa, so any further queries
    /// might return nonsense results. It is useful only in very specific
    /// circumstances -- for example, when one wants to observe which values
    /// dropped together with the table
    pub fn purge(&self)
    where
        Q::Storage: plumbing::QueryStorageMassOps,
    {
        self.storage.purge();
    }
}

/// Return value from [the `query_mut` method] on `Database`.
/// Gives access to the `set` method, notably, that is used to
/// set the value of an input query.
///
/// [the `query_mut` method]: trait.Database.html#method.query_mut
pub struct QueryTableMut<'me, Q>
where
    Q: Query + 'me,
{
    db: &'me mut <Q as QueryDb<'me>>::DynDb,
    storage: Arc<Q::Storage>,
}

impl<'me, Q> QueryTableMut<'me, Q>
where
    Q: Query,
{
    /// Constructs a new `QueryTableMut`.
    pub fn new(db: &'me mut <Q as QueryDb<'me>>::DynDb, storage: Arc<Q::Storage>) -> Self {
        Self { db, storage }
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
        Q::Storage: plumbing::InputQueryStorageOps<Q>,
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
        Q::Storage: plumbing::InputQueryStorageOps<Q>,
    {
        self.storage.set(self.db, &key, value, durability);
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
        Q::Storage: plumbing::DerivedQueryStorageOps<Q>,
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

impl CycleError<DatabaseKeyIndex> {
    fn debug<'a, D: ?Sized>(&'a self, db: &'a D) -> impl Debug + 'a
    where
        D: DatabaseOps,
    {
        struct CycleErrorDebug<'a, D: ?Sized> {
            db: &'a D,
            error: &'a CycleError<DatabaseKeyIndex>,
        }

        impl<'a, D: ?Sized + DatabaseOps> Debug for CycleErrorDebug<'a, D> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                writeln!(f, "Internal error, cycle detected:\n")?;
                for i in &self.error.cycle {
                    writeln!(f, "{:?}", i.debug(self.db))?;
                }
                Ok(())
            }
        }

        CycleErrorDebug { db, error: self }
    }
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

/// A panic payload indicating that a salsa revision was cancelled.
#[derive(Debug)]
#[non_exhaustive]
pub struct Cancelled;

impl Cancelled {
    fn throw() -> ! {
        // We use resume and not panic here to avoid running the panic
        // hook (that is, to avoid collecting and printing backtrace).
        std::panic::resume_unwind(Box::new(Self));
    }

    /// Runs `f`, and catches any salsa cancellation.
    pub fn catch<F, T>(f: F) -> Result<T, Cancelled>
    where
        F: FnOnce() -> T + UnwindSafe,
    {
        match panic::catch_unwind(f) {
            Ok(t) => Ok(t),
            Err(payload) => match payload.downcast() {
                Ok(cancelled) => Err(*cancelled),
                Err(payload) => panic::resume_unwind(payload),
            },
        }
    }
}

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

// Re-export the procedural macros.
#[allow(unused_imports)]
#[macro_use]
extern crate salsa_macros;
use plumbing::HasQueryGroup;
#[doc(hidden)]
pub use salsa_macros::*;
