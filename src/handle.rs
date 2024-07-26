use std::sync::Arc;

use parking_lot::{Condvar, Mutex};

use crate::{storage::HasStorage, Event, EventKind};

/// A database "handle" allows coordination of multiple async tasks accessing the same database.
/// So long as you are just doing reads, you can freely clone.
/// When you attempt to modify the database, you call `get_mut`, which will set the cancellation flag,
/// causing other handles to get panics. Once all other handles are dropped, you can proceed.
pub struct Handle<Db: HasStorage> {
    /// Reference to the database. This is always `Some` except during destruction.
    db: Option<Arc<Db>>,

    /// Coordination data.
    coordinate: Arc<Coordinate>,
}

struct Coordinate {
    /// Counter of the number of clones of actor. Begins at 1.
    /// Incremented when cloned, decremented when dropped.
    clones: Mutex<usize>,
    cvar: Condvar,
}

impl<Db: HasStorage> Handle<Db> {
    /// Create a new handle wrapping `db`.
    pub fn new(db: Db) -> Self {
        Self {
            db: Some(Arc::new(db)),
            coordinate: Arc::new(Coordinate {
                clones: Mutex::new(1),
                cvar: Default::default(),
            }),
        }
    }

    fn db(&self) -> &Arc<Db> {
        self.db.as_ref().unwrap()
    }

    fn db_mut(&mut self) -> &mut Arc<Db> {
        self.db.as_mut().unwrap()
    }

    /// Returns a mutable reference to the inner database.
    /// If other handles are active, this method sets the cancellation flag
    /// and blocks until they are dropped.
    pub fn get_mut(&mut self) -> &mut Db {
        self.cancel_others();

        // Once cancellation above completes, the other handles are being dropped.
        // However, because the signal is sent before the destructor completes, it's
        // possible that they have not *yet* dropped.
        //
        // Therefore, we may have to do a (short) bit of
        // spinning before we observe the thread-count reducing to 0.
        //
        // An alternative would be to
        Arc::get_mut(self.db_mut()).expect("other threads remain active despite cancellation")
    }

    /// Returns the inner database, consuming the handle.
    ///
    /// If other handles are active, this method sets the cancellation flag
    /// and blocks until they are dropped.
    pub fn into_inner(mut self) -> Db {
        self.cancel_others();
        Arc::into_inner(self.db.take().unwrap())
            .expect("other threads remain active despite cancellation")
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_others(&mut self) {
        let storage = self.db().storage();
        storage.runtime().set_cancellation_flag();

        self.db().salsa_event(Event {
            thread_id: std::thread::current().id(),

            kind: EventKind::DidSetCancellationFlag,
        });

        let mut clones = self.coordinate.clones.lock();
        while *clones != 1 {
            self.coordinate.cvar.wait(&mut clones);
        }
    }
    // ANCHOR_END: cancel_other_workers
}

impl<Db: HasStorage> Drop for Handle<Db> {
    fn drop(&mut self) {
        // Drop the database handle *first*
        self.db.take();

        // *Now* decrement the number of clones and notify once we have completed
        *self.coordinate.clones.lock() -= 1;
        self.coordinate.cvar.notify_all();
    }
}

impl<Db: HasStorage> std::ops::Deref for Handle<Db> {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        self.db()
    }
}

impl<Db: HasStorage> Clone for Handle<Db> {
    fn clone(&self) -> Self {
        *self.coordinate.clones.lock() += 1;

        Self {
            db: Some(Arc::clone(self.db())),
            coordinate: Arc::clone(&self.coordinate),
        }
    }
}
