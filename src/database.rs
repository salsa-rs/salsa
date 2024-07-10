use std::{any::Any, cell::Cell, ptr::NonNull};

use crossbeam::atomic::AtomicCell;
use parking_lot::Mutex;

use crate::{storage::DatabaseGen, Durability, Event};

pub trait Database: DatabaseGen {
    /// This function is invoked at key points in the salsa
    /// runtime. It permits the database to be customized and to
    /// inject logging or other custom behavior.
    ///
    /// By default, the event is logged at level debug using
    /// the standard `log` facade.
    fn salsa_event(&self, event: Event) {
        log::debug!("salsa_event: {:?}", event)
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

/// The database view trait allows you to define your own views on the database.
/// This lets you add extra context beyond what is stored in the salsa database itself.
pub trait DatabaseView<Dyn: ?Sized + Any>: Database {
    /// Registers this database view in the database.
    /// This is normally invoked automatically by tracked functions that require a given view.
    fn add_view_to_db(&self);
}

impl<Db: Database> DatabaseView<dyn Database> for Db {
    fn add_view_to_db(&self) {
        let upcasts = self.upcasts_for_self();
        upcasts.add::<dyn Database>(|t| t, |t| t);
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

thread_local! {
    static DATABASE: Cell<AttachedDatabase> = Cell::new(AttachedDatabase::null());
}

/// Access the "attached" database. Returns `None` if no database is attached.
/// Databases are attached with `attach_database`.
pub fn with_attached_database<R>(op: impl FnOnce(&dyn Database) -> R) -> Option<R> {
    // SAFETY: We always attach the database in for the entire duration of a function,
    // so it cannot become "unattached" while this function is running.
    let db = DATABASE.get();
    Some(op(unsafe { db.ptr?.as_ref() }))
}

/// Attach database and returns a guard that will un-attach the database when dropped.
/// Has no effect if a database is already attached.
pub fn attach_database<Db: ?Sized + Database, R>(db: &Db, op: impl FnOnce() -> R) -> R {
    let _guard = AttachedDb::new(db);
    op()
}

#[derive(Copy, Clone, PartialEq, Eq)]
struct AttachedDatabase {
    ptr: Option<NonNull<dyn Database>>,
}

impl AttachedDatabase {
    pub const fn null() -> Self {
        Self { ptr: None }
    }

    pub fn from<Db: ?Sized + Database>(db: &Db) -> Self {
        unsafe {
            let db: *const dyn Database = db.as_salsa_database();
            Self {
                ptr: Some(NonNull::new_unchecked(db as *mut dyn Database)),
            }
        }
    }
}

unsafe impl Send for AttachedDatabase where dyn Database: Sync {}

unsafe impl Sync for AttachedDatabase where dyn Database: Sync {}

struct AttachedDb<'db, Db: ?Sized + Database> {
    db: &'db Db,
    previous: AttachedDatabase,
}

impl<'db, Db: ?Sized + Database> AttachedDb<'db, Db> {
    pub fn new(db: &'db Db) -> Self {
        let previous = DATABASE.replace(AttachedDatabase::from(db));
        AttachedDb { db, previous }
    }
}

impl<Db: ?Sized + Database> Drop for AttachedDb<'_, Db> {
    fn drop(&mut self) {
        DATABASE.set(self.previous);
    }
}

impl<Db: ?Sized + Database> std::ops::Deref for AttachedDb<'_, Db> {
    type Target = Db;

    fn deref(&self) -> &Db {
        &self.db
    }
}
