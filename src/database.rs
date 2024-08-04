use std::{any::Any, borrow::Cow};

use crate::{
    zalsa::{IngredientIndex, ZalsaDatabase},
    Durability, Event, Revision,
};

/// The trait implemented by all Salsa databases.
/// You can create your own subtraits of this trait using the `#[salsa::db]`(`crate::db`) procedural macro.
#[crate::db]
pub trait Database: Send + AsDynDatabase + Any + ZalsaDatabase {
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
        let zalsa_local = db.zalsa_local();
        zalsa_local.report_untracked_read(db.zalsa().current_revision())
    }

    /// Return the "debug name" (i.e., the struct name, etc) for an "ingredient",
    /// which are the fine-grained components we use to track data. This is intended
    /// for debugging and the contents of the returned string are not semver-guaranteed.
    ///
    /// Ingredient indices can be extracted from [`DependencyIndex`](`crate::DependencyIndex`) values.
    fn ingredient_debug_name(&self, ingredient_index: IngredientIndex) -> Cow<'_, str> {
        Cow::Borrowed(
            self.zalsa()
                .lookup_ingredient(ingredient_index)
                .debug_name(),
        )
    }

    /// Execute `op` with the database in thread-local storage for debug print-outs.
    fn attach<R>(&self, op: impl FnOnce(&Self) -> R) -> R
    where
        Self: Sized,
    {
        crate::attach::attach(self, || op(self))
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
