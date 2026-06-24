use std::cell::Cell;
use std::ptr::NonNull;

use crate::Database;

#[cfg(feature = "shuttle")]
crate::sync::thread_local! {
    /// The thread-local state salsa requires for a given thread
    static ATTACHED: Attached = Attached::new();
}

// shuttle's `thread_local` macro does not support const-initialization.
#[cfg(not(feature = "shuttle"))]
crate::sync::thread_local! {
    /// The thread-local state salsa requires for a given thread
    static ATTACHED: Attached = const { Attached::new() }
}

/// State that is specific to a single execution thread.
///
/// Internally, this type uses ref-cells.
///
/// **Note also that all mutations to the database handle (and hence
/// to the local-state) must be undone during unwinding.**
struct Attached {
    /// Pointer to the currently attached database.
    database: Cell<Option<NonNull<dyn Database>>>,
}

impl Attached {
    const fn new() -> Self {
        Self {
            database: Cell::new(None),
        }
    }

    #[inline]
    fn is_current_database<Db>(&self, db: &Db) -> bool
    where
        Db: ?Sized + Database,
    {
        let Some(current_db) = self.database.get() else {
            return false;
        };

        let new_db = NonNull::from(db.as_dyn_database());
        if !std::ptr::addr_eq(current_db.as_ptr(), new_db.as_ptr()) {
            panic!("Cannot change database mid-query. current: {current_db:?}, new: {new_db:?}");
        }

        true
    }

    #[inline]
    fn attach<Db, R>(&self, db: &Db, op: impl FnOnce() -> R) -> R
    where
        Db: ?Sized + Database,
    {
        struct DbGuard<'s> {
            /// The database that *we* attached on scope entry.
            ///
            /// `None` if one was already attached by a parent scope.
            state: Option<&'s Attached>,
        }

        impl<'s> DbGuard<'s> {
            #[inline]
            fn new(attached: &'s Attached, db: &dyn Database) -> Self {
                if attached.is_current_database(db) {
                    Self { state: None }
                } else {
                    attached.database.set(Some(NonNull::from(db)));
                    db.zalsa_local().set_attached(true);
                    Self {
                        state: Some(attached),
                    }
                }
            }
        }

        impl Drop for DbGuard<'_> {
            #[inline]
            fn drop(&mut self) {
                // Reset database to null if we did anything in `DbGuard::new`.
                if let Some(attached) = self.state {
                    if let Some(prev) = attached.database.replace(None) {
                        // SAFETY: `prev` is a valid pointer to a database.
                        unsafe {
                            let zalsa_local = prev.as_ref().zalsa_local();
                            zalsa_local.set_attached(false);
                            zalsa_local.uncancel();
                        }
                    }
                }
            }
        }

        let _guard = DbGuard::new(self, db.as_dyn_database());
        op()
    }

    #[inline]
    fn attach_allow_change<Db, R>(&self, db: &Db, op: impl FnOnce() -> R) -> R
    where
        Db: ?Sized + Database,
    {
        struct DbGuard<'s> {
            /// The database that *we* attached on scope entry.
            ///
            /// `None` if one was already attached by a parent scope.
            state: Option<&'s Attached>,
            /// The previously attached database that we replaced, if any.
            ///
            /// We need to make sure to rollback and activate it again when we exit the scope.
            prev: Option<NonNull<dyn Database>>,
        }

        impl<'s> DbGuard<'s> {
            #[inline]
            fn new(attached: &'s Attached, db: &dyn Database) -> Self {
                let db = NonNull::from(db);
                match attached.database.replace(Some(db)) {
                    // A database was already attached by a parent scope.
                    Some(prev) => {
                        if std::ptr::eq(db.as_ptr(), prev.as_ptr()) {
                            // and it was the same as ours, so we did not change anything.
                            Self {
                                state: None,
                                prev: None,
                            }
                        } else {
                            // and it was the a different one from ours, record the state changes.
                            // SAFETY: Both pointers remain valid for their attachment scopes.
                            unsafe {
                                prev.as_ref().zalsa_local().set_attached(false);
                                db.as_ref().zalsa_local().set_attached(true);
                            }
                            Self {
                                state: Some(attached),
                                prev: Some(prev),
                            }
                        }
                    }
                    // No database is attached, attach the new one.
                    None => {
                        attached.database.set(Some(db));
                        // SAFETY: `db` remains valid for the attachment scope.
                        unsafe { db.as_ref().zalsa_local().set_attached(true) };
                        Self {
                            state: Some(attached),
                            prev: None,
                        }
                    }
                }
            }
        }

        impl Drop for DbGuard<'_> {
            #[inline]
            fn drop(&mut self) {
                // Reset database to null if we did anything in `DbGuard::new`.
                if let Some(attached) = self.state {
                    if let Some(prev) = attached.database.replace(self.prev) {
                        // SAFETY: `prev` is a valid pointer to a database.
                        unsafe {
                            let zalsa_local = prev.as_ref().zalsa_local();
                            zalsa_local.set_attached(false);
                            zalsa_local.uncancel();
                        }
                    }
                    if let Some(prev) = self.prev {
                        // SAFETY: `prev` is valid for its enclosing attachment scope.
                        unsafe { prev.as_ref().zalsa_local().set_attached(true) };
                    }
                }
            }
        }

        let _guard = DbGuard::new(self, db.as_dyn_database());
        op()
    }

    /// Access the "attached" database. Returns `None` if no database is attached.
    /// Databases are attached with `attach_database`.
    #[inline]
    fn with<R>(&self, op: impl FnOnce(&dyn Database) -> R) -> Option<R> {
        let db = self.database.get()?;

        // SAFETY: We always attach the database in for the entire duration of a function,
        // so it cannot become "unattached" while this function is running.
        Some(op(unsafe { db.as_ref() }))
    }
}

/// Attach the database to the current thread and execute `op`.
/// Panics if a different database has already been attached.
#[inline]
pub fn attach<R, Db>(db: &Db, op: impl FnOnce() -> R) -> R
where
    Db: ?Sized + Database,
{
    ATTACHED.with(
        #[inline]
        |a| a.attach(db, op),
    )
}

/// Panics if a database other than `db` is currently attached.
#[doc(hidden)]
#[inline]
pub fn assert_current_database<Db>(db: &Db)
where
    Db: ?Sized + Database,
{
    ATTACHED.with(
        #[inline]
        |attached| {
            attached.is_current_database(db);
        },
    )
}

/// Panics if a database other than `db` is currently attached, unless `zalsa_local` already
/// records that `db` is attached.
#[doc(hidden)]
#[inline(always)]
pub fn assert_current_database_or_attached<Db>(
    db: &Db,
    zalsa_local: &crate::zalsa_local::ZalsaLocal,
) where
    Db: ?Sized + Database,
{
    if !zalsa_local.is_attached() {
        assert_current_database_unattached(db);
    }
}

#[cold]
#[inline(never)]
fn assert_current_database_unattached<Db>(db: &Db)
where
    Db: ?Sized + Database,
{
    assert_current_database(db);
}

#[inline]
pub(crate) fn attach_if_needed<R, Db>(db: &Db, op: impl FnOnce() -> R) -> R
where
    Db: ?Sized + Database,
{
    ATTACHED.with(
        #[inline]
        |attached| {
            if attached.is_current_database(db) {
                op()
            } else {
                attach_cold(attached, db, op)
            }
        },
    )
}

#[cold]
#[inline(never)]
fn attach_cold<R, Db>(attached: &Attached, db: &Db, op: impl FnOnce() -> R) -> R
where
    Db: ?Sized + Database,
{
    attached.attach(db, op)
}

#[inline]
pub(crate) fn is_attached<Db>(db: &Db) -> bool
where
    Db: ?Sized + Database,
{
    ATTACHED.with(
        #[inline]
        |attached| {
            attached.database.get().is_some_and(|current_db| {
                let db = NonNull::from(db.as_dyn_database());
                std::ptr::addr_eq(current_db.as_ptr(), db.as_ptr())
            })
        },
    )
}

/// Attach the database to the current thread and execute `op`.
/// Allows a different database than currently attached. The original database
/// will be restored on return.
///
/// **Note:** Switching databases can cause bugs. If you do not intend to switch
/// databases, prefer [`attach`] which will panic if you accidentally do.
#[inline]
pub fn attach_allow_change<R, Db>(db: &Db, op: impl FnOnce() -> R) -> R
where
    Db: ?Sized + Database,
{
    ATTACHED.with(
        #[inline]
        |a| a.attach_allow_change(db, op),
    )
}

/// Access the "attached" database. Returns `None` if no database is attached.
/// Databases are attached with `attach_database`.
#[inline]
pub fn with_attached_database<R>(op: impl FnOnce(&dyn Database) -> R) -> Option<R> {
    ATTACHED.with(
        #[inline]
        |a| a.with(op),
    )
}
