use std::{marker::PhantomData, panic::RefUnwindSafe, sync::Arc};

use parking_lot::{Condvar, Mutex};

use crate::{
    plumbing::{input, interned, tracked_struct},
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
pub unsafe trait HasStorage: Database + Clone + Sized {
    fn storage(&self) -> &Storage<Self>;
    fn storage_mut(&mut self) -> &mut Storage<Self>;
}

/// Concrete implementation of the [`Database`][] trait.
/// Takes an optional type parameter `U` that allows you to thread your own data.
pub struct Storage<Db: Database> {
    // Note: Drop order is important, zalsa_impl needs to drop before coordinate
    /// Reference to the database.
    zalsa_impl: Arc<Zalsa>,

    // Note: Drop order is important, coordinate needs to drop after zalsa_impl
    /// Coordination data for cancellation of other handles when `zalsa_mut` is called.
    /// This could be stored in Zalsa but it makes things marginally cleaner to keep it separate.
    coordinate: CoordinateDrop,

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
            zalsa_impl: Arc::new(Zalsa::new::<Db>()),
            coordinate: CoordinateDrop(Arc::new(Coordinate {
                clones: Mutex::new(1),
                cvar: Default::default(),
            })),
            zalsa_local: ZalsaLocal::new(),
            phantom: PhantomData,
        }
    }
}

impl<Db: Database> Storage<Db> {
    pub fn debug_input_entries<T>(&self) -> impl Iterator<Item = &input::Value<T>>
    where
        T: input::Configuration,
    {
        let zalsa = self.zalsa_impl();
        zalsa
            .table()
            .pages
            .iter()
            .filter_map(|page| page.cast_type::<crate::table::Page<input::Value<T>>>())
            .flat_map(|page| page.slots())
    }

    pub fn debug_interned_entries<T>(&self) -> impl Iterator<Item = &interned::Value<T>>
    where
        T: interned::Configuration,
    {
        let zalsa = self.zalsa_impl();
        zalsa
            .table()
            .pages
            .iter()
            .filter_map(|page| page.cast_type::<crate::table::Page<interned::Value<T>>>())
            .flat_map(|page| page.slots())
    }

    pub fn debug_tracked_entries<T>(&self) -> impl Iterator<Item = &tracked_struct::Value<T>>
    where
        T: tracked_struct::Configuration,
    {
        let zalsa = self.zalsa_impl();
        zalsa
            .table()
            .pages
            .iter()
            .filter_map(|page| page.cast_type::<crate::table::Page<tracked_struct::Value<T>>>())
            .flat_map(|pages| pages.slots())
    }

    /// Access the `Arc<Zalsa>`. This should always be
    /// possible as `zalsa_impl` only becomes
    /// `None` once we are in the `Drop` impl.
    fn zalsa_impl(&self) -> &Arc<Zalsa> {
        &self.zalsa_impl
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_others(&self, db: &Db) {
        self.zalsa_impl.set_cancellation_flag();

        db.salsa_event(&|| Event::new(EventKind::DidSetCancellationFlag));

        let mut clones = self.coordinate.clones.lock();
        while *clones != 1 {
            self.coordinate.cvar.wait(&mut clones);
        }
    }
    // ANCHOR_END: cancel_other_workers
}

unsafe impl<T: HasStorage> ZalsaDatabase for T {
    fn zalsa(&self) -> &Zalsa {
        &self.storage().zalsa_impl
    }

    fn zalsa_mut(&mut self) -> &mut Zalsa {
        self.storage().cancel_others(self);

        let storage = self.storage_mut();
        // The ref count on the `Arc` should now be 1
        let zalsa_mut = Arc::get_mut(&mut storage.zalsa_impl).unwrap();
        zalsa_mut.new_revision();
        zalsa_mut
    }

    fn zalsa_local(&self) -> &ZalsaLocal {
        &self.storage().zalsa_local
    }

    fn fork_db(&self) -> Box<dyn Database> {
        Box::new(self.clone())
    }
}

impl<Db: Database> RefUnwindSafe for Storage<Db> {}

impl<Db: Database> Clone for Storage<Db> {
    fn clone(&self) -> Self {
        *self.coordinate.clones.lock() += 1;

        Self {
            zalsa_impl: self.zalsa_impl.clone(),
            coordinate: CoordinateDrop(Arc::clone(&self.coordinate)),
            zalsa_local: ZalsaLocal::new(),
            phantom: PhantomData,
        }
    }
}

struct CoordinateDrop(Arc<Coordinate>);

impl std::ops::Deref for CoordinateDrop {
    type Target = Arc<Coordinate>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for CoordinateDrop {
    fn drop(&mut self) {
        *self.0.clones.lock() -= 1;
        self.0.cvar.notify_all();
    }
}
