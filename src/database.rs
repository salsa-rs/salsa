use crate::{storage::HasJarsDyn, DebugWithDb, Durability, Event};

pub trait Database: HasJarsDyn + AsSalsaDatabase {
    /// This function is invoked at key points in the salsa
    /// runtime. It permits the database to be customized and to
    /// inject logging or other custom behavior.
    ///
    /// By default, the event is logged at level debug using
    /// the standard `log` facade.
    fn salsa_event(&self, event: Event) {
        log::debug!("salsa_event: {:?}", event.debug(self));
    }

    /// A "synthetic write" causes the system to act *as though* some
    /// input of durability `durability` has changed. This is mostly
    /// useful for profiling scenarios.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn synthetic_write(&mut self, durability: Durability) {
        self.runtime_mut().report_tracked_write(durability);
    }

    /// Reports that the query depends on some state unknown to salsa.
    ///
    /// Queries which report untracked reads will be re-executed in the next
    /// revision.
    fn report_untracked_read(&self) {
        self.runtime().report_untracked_read();
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
    ///                 runtime: self.storage.snapshot(),
    ///                 other_field: self.other_field.clone(),
    ///             }
    ///         )
    ///     }
    /// }
    /// ```
    fn snapshot(&self) -> Snapshot<Self>;
}
pub trait AsSalsaDatabase {
    fn as_salsa_database(&self) -> &dyn Database;
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
