use crate::{local_state, storage::ZalsaDatabase, Durability, Event, Revision};

#[salsa_macros::db]
pub trait Database: ZalsaDatabase + AsDynDatabase {
    /// This function is invoked at key points in the salsa
    /// runtime. It permits the database to be customized and to
    /// inject logging or other custom behavior.
    ///
    /// By default, the event is logged at level debug using
    /// the standard `log` facade.
    fn salsa_event(&self, event: Event) {
        tracing::debug!("salsa_event: {:?}", event)
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
        let runtime = self.zalsa_mut().runtime_mut();
        runtime.new_revision();
        runtime.report_tracked_write(durability);
    }

    /// Reports that the query depends on some state unknown to salsa.
    ///
    /// Queries which report untracked reads will be re-executed in the next
    /// revision.
    fn report_untracked_read(&self) {
        let db = self.as_dyn_database();
        local_state::attach(db, |state| {
            state.report_untracked_read(db.zalsa().runtime().current_revision())
        })
    }

    /// Execute `op` with the database in thread-local storage for debug print-outs.
    fn attach<R>(&self, op: impl FnOnce(&Self) -> R) -> R
    where
        Self: Sized,
    {
        local_state::attach(self, |_state| op(self))
    }
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
    db.zalsa().runtime().current_revision()
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
