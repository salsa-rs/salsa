use std::sync::Arc;

use parking_lot::{Condvar, Mutex};

use crate::storage::HasStorage;

/// A database "handle" allows coordination of multiple async tasks accessing the same database.
/// So long as you are just doing reads, you can freely clone.
/// When you attempt to modify the database, you call `get_mut`, which will set the cancellation flag,
/// causing other handles to get panics. Once all other handles are dropped, you can proceed.
pub struct Handle<Db: HasStorage> {
    db: Arc<Db>,
    coordinate: Arc<Coordinate>,
}

struct Coordinate {
    /// Counter of the number of clones of actor. Begins at 1.
    /// Incremented when cloned, decremented when dropped.
    clones: Mutex<usize>,
    cvar: Condvar,
}

impl<Db: HasStorage> Handle<Db> {
    pub fn new(db: Db) -> Self {
        Self {
            db: Arc::new(db),
            coordinate: Arc::new(Coordinate {
                clones: Mutex::new(1),
                cvar: Default::default(),
            }),
        }
    }

    /// Returns a mutable reference to the inner database.
    /// If other handles are active, this method sets the cancellation flag
    /// and blocks until they are dropped.
    pub fn get_mut(&mut self) -> &mut Db {
        self.cancel_others();
        Arc::get_mut(&mut self.db).expect("no other handles")
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_others(&mut self) {
        let storage = self.db.storage();
        storage.runtime().set_cancellation_flag();

        let mut clones = self.coordinate.clones.lock();
        while *clones != 1 {
            self.coordinate.cvar.wait(&mut clones);
        }
    }
    // ANCHOR_END: cancel_other_workers
}

impl<Db: HasStorage> Drop for Handle<Db> {
    fn drop(&mut self) {
        *self.coordinate.clones.lock() -= 1;
        self.coordinate.cvar.notify_all();
    }
}

impl<Db: HasStorage> std::ops::Deref for Handle<Db> {
    type Target = Db;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl<Db: HasStorage> Clone for Handle<Db> {
    fn clone(&self) -> Self {
        *self.coordinate.clones.lock() += 1;

        Self {
            db: Arc::clone(&self.db),
            coordinate: Arc::clone(&self.coordinate),
        }
    }
}
