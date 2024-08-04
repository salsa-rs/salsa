use std::{marker::PhantomData, panic::RefUnwindSafe, sync::Arc};

use parking_lot::{Condvar, Mutex};

use crate::{
    zalsa::{Zalsa, ZalsaDatabase},
    zalsa_local::{self, ZalsaLocal},
    Database, Event, EventKind,
};

/// Access the "storage" of a Salsa database: this is an internal plumbing trait
/// automatically implemented by `#[salsa::db]` applied to a struct.
///
/// # Safety
///
/// The `storage` and `storage_mut` fields must both return a reference to the same
/// storage field which must be owned by `self`.
pub unsafe trait HasStorage: Database + Sized {
    fn storage(&self) -> &Storage<Self>;
    fn storage_mut(&mut self) -> &mut Storage<Self>;
}

/// Concrete implementation of the [`Database`][] trait.
/// Takes an optional type parameter `U` that allows you to thread your own data.
pub struct Storage<Db: Database> {
    /// Reference to the database. This is always `Some` except during destruction.
    zalsa_impl: Option<Arc<Zalsa>>,

    /// Coordination data for cancellation of other handles when `zalsa_mut` is called.
    /// This could be stored in Zalsa but it makes things marginally cleaner to keep it separate.
    coordinate: Arc<Coordinate>,

    /// Per-thread state
    zalsa_local: zalsa_local::ZalsaLocal,

    /// We store references to `Db`
    phantom: PhantomData<fn() -> Db>,
}
struct Coordinate {
    /// Counter of the number of clones of actor. Begins at 1.
    /// Incremented when cloned, decremented when dropped.
    clones: Mutex<usize>,
    cvar: Condvar,
}

impl<Db: Database> Default for Storage<Db> {
    fn default() -> Self {
        Self {
            zalsa_impl: Some(Arc::new(Zalsa::new::<Db>())),
            coordinate: Arc::new(Coordinate {
                clones: Mutex::new(1),
                cvar: Default::default(),
            }),
            zalsa_local: ZalsaLocal::new(),
            phantom: PhantomData,
        }
    }
}

impl<Db: Database> Storage<Db> {
    /// Access the `Arc<Zalsa>`. This should always be
    /// possible as `zalsa_impl` only becomes
    /// `None` once we are in the `Drop` impl.
    fn zalsa_impl(&self) -> &Arc<Zalsa> {
        self.zalsa_impl.as_ref().unwrap()
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_others(&self, db: &Db) {
        let zalsa = self.zalsa_impl();
        zalsa.set_cancellation_flag();

        db.salsa_event(&|| Event {
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

unsafe impl<T: HasStorage> ZalsaDatabase for T {
    fn zalsa(&self) -> &Zalsa {
        self.storage().zalsa_impl.as_ref().unwrap()
    }

    fn zalsa_mut(&mut self) -> &mut Zalsa {
        self.storage().cancel_others(self);

        // The ref count on the `Arc` should now be 1
        let storage = self.storage_mut();
        let arc_zalsa_mut = storage.zalsa_impl.as_mut().unwrap();
        let zalsa_mut = Arc::get_mut(arc_zalsa_mut).unwrap();
        zalsa_mut.new_revision();
        zalsa_mut
    }

    fn zalsa_local(&self) -> &ZalsaLocal {
        &self.storage().zalsa_local
    }
}

impl<Db: Database> RefUnwindSafe for Storage<Db> {}

impl<Db: Database> Clone for Storage<Db> {
    fn clone(&self) -> Self {
        *self.coordinate.clones.lock() += 1;

        Self {
            zalsa_impl: self.zalsa_impl.clone(),
            coordinate: Arc::clone(&self.coordinate),
            zalsa_local: ZalsaLocal::new(),
            phantom: PhantomData,
        }
    }
}

impl<Db: Database> Drop for Storage<Db> {
    fn drop(&mut self) {
        // Drop the database handle *first*
        self.zalsa_impl.take();

        // *Now* decrement the number of clones and notify once we have completed
        *self.coordinate.clones.lock() -= 1;
        self.coordinate.cvar.notify_all();
    }
}
