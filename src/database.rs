use std::{cell::Cell, ptr::NonNull};

use crate::{storage::DatabaseGen, Durability, Event, Revision};

#[salsa_macros::db]
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

    /// Execute `op` with the database in thread-local storage for debug print-outs.
    fn attach<R>(&self, op: impl FnOnce(&Self) -> R) -> R
    where
        Self: Sized,
    {
        attach_database(self, || op(self))
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

pub fn current_revision<Db: ?Sized + Database>(db: &Db) -> Revision {
    db.runtime().current_revision()
}
