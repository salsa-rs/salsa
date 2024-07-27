use std::{any::Any, panic::RefUnwindSafe, sync::Arc};

use parking_lot::{Condvar, Mutex};

use crate::{
    self as salsa, local_state,
    storage::{Zalsa, ZalsaImpl},
    Durability, Event, EventKind, Revision,
};

/// The trait implemented by all Salsa databases.
/// You can create your own subtraits of this trait using the `#[salsa::db]` procedural macro.
///
/// # Safety conditions
///
/// This trait can only safely be implemented by Salsa's [`DatabaseImpl`][] type.
/// FIXME: Document better the unsafety conditions we guarantee.
#[salsa_macros::db]
pub unsafe trait Database: AsDynDatabase + Any {
    /// This function is invoked by the salsa runtime at various points during execution.
    /// You can customize what happens by implementing the [`UserData`][] trait.
    /// By default, the event is logged at level debug using tracing facade.
    ///
    /// # Parameters
    ///
    /// * `event`, a fn that, if called, will create the event that occurred
    fn salsa_event(&self, event: &dyn Fn() -> Event);

    /// A "synthetic write" causes the system to act *as though* some
    /// input of durability `durability` has changed. This is mostly
    /// useful for profiling scenarios.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn synthetic_write(&mut self, durability: Durability) {
        let zalsa_mut = self.zalsa_mut();
        zalsa_mut.report_tracked_write(durability);
    }

    /// Reports that the query depends on some state unknown to salsa.
    ///
    /// Queries which report untracked reads will be re-executed in the next
    /// revision.
    fn report_untracked_read(&self) {
        let db = self.as_dyn_database();
        local_state::attach(db, |state| {
            state.report_untracked_read(db.zalsa().current_revision())
        })
    }

    /// Execute `op` with the database in thread-local storage for debug print-outs.
    fn attach<R>(&self, op: impl FnOnce(&Self) -> R) -> R
    where
        Self: Sized,
    {
        local_state::attach(self, |_state| op(self))
    }

    /// Plumbing method: Access the internal salsa methods.
    #[doc(hidden)]
    fn zalsa(&self) -> &dyn Zalsa;

    /// Plumbing method: Access the internal salsa methods for mutating the database.
    ///
    /// **WARNING:** Triggers a new revision, canceling other database handles.
    /// This can lead to deadlock!
    #[doc(hidden)]
    fn zalsa_mut(&mut self) -> &mut dyn Zalsa;
}

/// Upcast to a `dyn Database`.
///
/// Only required because upcasts not yet stabilized (*grr*).
pub trait AsDynDatabase {
    fn as_dyn_database(&self) -> &dyn Database;
    fn as_dyn_database_mut(&mut self) -> &mut dyn Database;
}

impl<T: Database> AsDynDatabase for T {
    fn as_dyn_database(&self) -> &dyn Database {
        self
    }

    fn as_dyn_database_mut(&mut self) -> &mut dyn Database {
        self
    }
}

pub fn current_revision<Db: ?Sized + Database>(db: &Db) -> Revision {
    db.zalsa().current_revision()
}

impl dyn Database {
    /// Upcasts `self` to the given view.
    ///
    /// # Panics
    ///
    /// If the view has not been added to the database (see [`DatabaseView`][])
    #[track_caller]
    pub fn as_view<DbView: ?Sized + Database>(&self) -> &DbView {
        self.zalsa().views().try_view_as(self).unwrap()
    }
}

/// Concrete implementation of the [`Database`][] trait.
/// Takes an optional type parameter `U` that allows you to thread your own data.
pub struct DatabaseImpl<U: UserData = ()> {
    /// Reference to the database. This is always `Some` except during destruction.
    zalsa_impl: Option<Arc<ZalsaImpl<U>>>,

    /// Coordination data.
    coordinate: Arc<Coordinate>,
}

impl<U: UserData + Default> Default for DatabaseImpl<U> {
    fn default() -> Self {
        Self::with(U::default())
    }
}

impl DatabaseImpl<()> {
    /// Create a new database with the given user data.
    ///
    /// You can also use the [`Default`][] trait if your userdata implements it.
    pub fn new() -> Self {
        Self::with(())
    }
}

impl<U: UserData> DatabaseImpl<U> {
    /// Create a new database with the given user data.
    ///
    /// You can also use the [`Default`][] trait if your userdata implements it.
    pub fn with(u: U) -> Self {
        Self {
            zalsa_impl: Some(Arc::new(ZalsaImpl::with(u))),
            coordinate: Arc::new(Coordinate {
                clones: Mutex::new(1),
                cvar: Default::default(),
            }),
        }
    }

    fn zalsa_impl(&self) -> &Arc<ZalsaImpl<U>> {
        self.zalsa_impl.as_ref().unwrap()
    }

    // ANCHOR: cancel_other_workers
    /// Sets cancellation flag and blocks until all other workers with access
    /// to this storage have completed.
    ///
    /// This could deadlock if there is a single worker with two handles to the
    /// same database!
    fn cancel_others(&mut self) {
        let zalsa = self.zalsa_impl();
        zalsa.set_cancellation_flag();

        self.salsa_event(&|| Event {
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

impl<U: UserData> std::ops::Deref for DatabaseImpl<U> {
    type Target = U;

    fn deref(&self) -> &U {
        self.zalsa_impl().user_data()
    }
}

impl<U: UserData + RefUnwindSafe> RefUnwindSafe for DatabaseImpl<U> {}

#[salsa_macros::db]
unsafe impl<U: UserData> Database for DatabaseImpl<U> {
    fn zalsa(&self) -> &dyn Zalsa {
        &**self.zalsa_impl()
    }

    fn zalsa_mut(&mut self) -> &mut dyn Zalsa {
        self.cancel_others();

        // The ref count on the `Arc` should now be 1
        let arc_zalsa_mut = self.zalsa_impl.as_mut().unwrap();
        let zalsa_mut = Arc::get_mut(arc_zalsa_mut).unwrap();
        zalsa_mut.new_revision();
        zalsa_mut
    }

    // Report a salsa event.
    fn salsa_event(&self, event: &dyn Fn() -> Event) {
        U::salsa_event(self, event)
    }
}

impl<U: UserData> Clone for DatabaseImpl<U> {
    fn clone(&self) -> Self {
        *self.coordinate.clones.lock() += 1;

        Self {
            zalsa_impl: self.zalsa_impl.clone(),
            coordinate: Arc::clone(&self.coordinate),
        }
    }
}

impl<U: UserData> Drop for DatabaseImpl<U> {
    fn drop(&mut self) {
        // Drop the database handle *first*
        self.zalsa_impl.take();

        // *Now* decrement the number of clones and notify once we have completed
        *self.coordinate.clones.lock() -= 1;
        self.coordinate.cvar.notify_all();
    }
}

pub trait UserData: Any + Sized {
    /// Callback invoked by the [`Database`][] at key points during salsa execution.
    /// By overriding this method, you can inject logging or other custom behavior.
    ///
    /// By default, the event is logged at level debug using the `tracing` crate.
    ///
    /// # Parameters
    ///
    /// * `event` a fn that, if called, will return the event that occurred
    fn salsa_event(_db: &DatabaseImpl<Self>, event: &dyn Fn() -> Event) {
        tracing::debug!("salsa_event: {:?}", event())
    }
}

impl UserData for () {}

struct Coordinate {
    /// Counter of the number of clones of actor. Begins at 1.
    /// Incremented when cloned, decremented when dropped.
    clones: Mutex<usize>,
    cvar: Condvar,
}
