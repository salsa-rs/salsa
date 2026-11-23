use std::cell::Cell;
use std::ptr::NonNull;

use crate::{Database, zalsa::Zalsa, zalsa_local::ZalsaLocal};

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
/// **Note also that all mutations to the database handle (and hence
/// to the local-state) must be undone during unwinding.**
struct Attached {
    /// The active attachment borrows both the database and its local storage.
    database: Cell<Option<Handle>>,
}

#[derive(Clone, Copy)]
struct Handle {
    local: NonNull<ZalsaLocal>,
    db: NonNull<dyn Database>,
}

/// Clears an attachment installed on a previously empty thread.
struct RootGuard<'db>(&'db ZalsaLocal);

impl Drop for RootGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        ATTACHED.with(|a| a.database.set(None));
        self.0.uncancel();
    }
}

impl Attached {
    const fn new() -> Self {
        Self {
            database: Cell::new(None),
        }
    }

    #[inline]
    fn attach<'db, Db: ?Sized + Database>(
        &self,
        db: &'db Db,
        local: &'db ZalsaLocal,
    ) -> Option<RootGuard<'db>> {
        // HasStorage requires an owned Storage<Self>. Its ZalsaLocal is unique
        // even when two databases of different types share a data address.
        // A borrowed local is never null, so one comparison covers an empty slot.
        let current = self
            .database
            .get()
            .map_or(std::ptr::null(), |handle| handle.local.as_ptr());
        if std::ptr::eq(current, local) {
            return None;
        }

        std::hint::cold_path();
        let db = NonNull::from(db.as_dyn_database());
        if let Some(current) = self.database.get() {
            panic!(
                "Cannot change database mid-query. current: {:?}, new: {db:?}",
                current.db
            );
        }
        self.database.set(Some(Handle {
            db,
            local: NonNull::from(local),
        }));
        Some(RootGuard(local))
    }

    #[inline]
    fn attach_allow_change<Db, R>(&self, db: &Db, op: impl FnOnce() -> R) -> R
    where
        Db: ?Sized + Database,
    {
        struct DbGuard<'s> {
            attached: &'s Attached,
            prev: Option<Handle>,
        }

        impl Drop for DbGuard<'_> {
            #[inline]
            fn drop(&mut self) {
                if let Some(current) = self.attached.database.replace(self.prev) {
                    // SAFETY: The database is borrowed for the duration of this scope.
                    unsafe { current.local.as_ref().uncancel() };
                }
            }
        }

        // The same database can have multiple vtables. A database and a field
        // of a different type can share an address.
        let same_db = self
            .with(|current| std::ptr::addr_eq(current, db) && current.type_id() == db.type_id())
            .unwrap_or(false);
        if same_db {
            return op();
        }

        let db = Handle {
            db: NonNull::from(db.as_dyn_database()),
            local: NonNull::from(db.zalsa_local()),
        };
        let _guard = DbGuard {
            attached: self,
            prev: self.database.replace(Some(db)),
        };
        op()
    }

    /// Access the "attached" database. Returns `None` if no database is attached.
    /// Databases are attached with `attach_database`.
    #[inline]
    fn with<R>(&self, op: impl FnOnce(&dyn Database) -> R) -> Option<R> {
        let db = self.database.get()?.db;

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
    let _guard = ATTACHED.with(|a| a.attach(db, db.zalsa_local()));
    op()
}

/// Attach a tracked function using the storage references needed by the query.
#[inline]
pub fn attach_query<'db, R, Db: ?Sized + Database>(
    db: &'db Db,
    op: impl FnOnce(&'db Zalsa, &'db ZalsaLocal) -> R,
) -> R {
    let (zalsa, local) = db.zalsas();
    let _guard = ATTACHED.with(|a| a.attach(db, local));
    op(zalsa, local)
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
