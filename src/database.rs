use std::any::Any;
use std::borrow::Cow;

use crate::views::DatabaseDownCaster;
use crate::zalsa::{IngredientIndex, ZalsaDatabase};
use crate::{Durability, Revision};

/// The trait implemented by all Salsa databases.
/// You can create your own subtraits of this trait using the `#[salsa::db]`(`crate::db`) procedural macro.
pub trait Database: Send + AsDynDatabase + Any + ZalsaDatabase {
    /// Enforces current LRU limits, evicting entries if necessary.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn trigger_lru_eviction(&mut self) {
        let zalsa_mut = self.zalsa_mut();
        zalsa_mut.evict_lru();
    }

    /// A "synthetic write" causes the system to act *as though* some
    /// input of durability `durability` has changed, triggering a new revision.
    /// This is mostly useful for profiling scenarios.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn synthetic_write(&mut self, durability: Durability) {
        let zalsa_mut = self.zalsa_mut();
        zalsa_mut.new_revision();
        zalsa_mut.runtime_mut().report_tracked_write(durability);
    }

    /// Reports that the query depends on some state unknown to salsa.
    ///
    /// Queries which report untracked reads will be re-executed in the next
    /// revision.
    fn report_untracked_read(&self) {
        let (zalsa, zalsa_local) = self.zalsas();
        zalsa_local.report_untracked_read(zalsa.current_revision())
    }

    /// Return the "debug name" (i.e., the struct name, etc) for an "ingredient",
    /// which are the fine-grained components we use to track data. This is intended
    /// for debugging and the contents of the returned string are not semver-guaranteed.
    ///
    /// Ingredient indices can be extracted from [`DatabaseKeyIndex`](`crate::DatabaseKeyIndex`) values.
    fn ingredient_debug_name(&self, ingredient_index: IngredientIndex) -> Cow<'_, str> {
        Cow::Borrowed(
            self.zalsa()
                .lookup_ingredient(ingredient_index)
                .debug_name(),
        )
    }

    /// Starts unwinding the stack if the current revision is cancelled.
    ///
    /// This method can be called by query implementations that perform
    /// potentially expensive computations, in order to speed up propagation of
    /// cancellation.
    ///
    /// Cancellation will automatically be triggered by salsa on any query
    /// invocation.
    ///
    /// This method should not be overridden by `Database` implementors. A
    /// `salsa_event` is emitted when this method is called, so that should be
    /// used instead.
    fn unwind_if_revision_cancelled(&self) {
        let (zalsa, zalsa_local) = self.zalsas();
        zalsa.unwind_if_revision_cancelled(zalsa_local);
    }

    /// Execute `op` with the database in thread-local storage for debug print-outs.
    #[inline(always)]
    fn attach<R>(&self, op: impl FnOnce(&Self) -> R) -> R
    where
        Self: Sized,
    {
        crate::attach::attach(self, || op(self))
    }

    #[cold]
    #[inline(never)]
    #[doc(hidden)]
    fn zalsa_register_downcaster(&self) -> DatabaseDownCaster<dyn Database> {
        self.zalsa().views().downcaster_for::<dyn Database>()
        // The no-op downcaster is special cased in view caster construction.
    }

    #[doc(hidden)]
    #[inline(always)]
    unsafe fn downcast(db: &dyn Database) -> &dyn Database
    where
        Self: Sized,
    {
        // No-op
        db
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
    #[inline(always)]
    fn as_dyn_database(&self) -> &dyn Database {
        self
    }

    #[inline(always)]
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
    /// If the view has not been added to the database (see [`crate::views::Views`]).
    #[track_caller]
    pub fn as_view<DbView: ?Sized + Database>(&self) -> &DbView {
        let views = self.zalsa().views();
        views.downcaster_for().downcast(self)
    }
}

#[cfg(feature = "salsa_unstable")]
pub use memory_usage::{MemoMemoryInfo, MemoryUsageVisitor, StructMemoryInfo};

#[cfg(feature = "salsa_unstable")]
mod memory_usage {
    use crate::Database;

    pub trait MemoryUsageVisitor: std::any::Any {
        fn visit_tracked_struct(&mut self, info: StructMemoryInfo) {
            self.visit_struct(info);
        }

        fn visit_memo(&mut self, info: MemoMemoryInfo) {
            let _ = info;
        }

        fn visit_input_struct(&mut self, info: StructMemoryInfo) {
            self.visit_struct(info);
        }

        fn visit_interned_struct(&mut self, info: StructMemoryInfo) {
            self.visit_struct(info);
        }

        fn visit_struct(&mut self, info: StructMemoryInfo) {
            let _ = info;
        }

        fn add_detail(&mut self, name: &'static str, size: usize) {
            let (_, _) = (name, size);
        }
    }

    impl dyn Database {
        /// Collects information about the memory usage of salsa structs and query functions.
        pub fn memory_usage(&self, visitor: &mut dyn MemoryUsageVisitor) {
            for ingredient in self.zalsa().ingredients() {
                ingredient.memory_usage(self, visitor);
            }
        }
    }

    /// Memory usage information about a particular instance of struct, input, output, or memo.
    #[derive(Debug, PartialEq, Eq)]
    pub struct MemoMemoryInfo {
        pub(crate) query_debug_name: &'static str,
        pub(crate) result_debug_name: &'static str,
        pub(crate) size_of_metadata: usize,
        pub(crate) size_of_fields: usize,
        pub(crate) heap_size_of_fields: Option<usize>,
    }

    impl MemoMemoryInfo {
        /// Returns the debug name of the ingredient.
        pub fn query_debug_name(&self) -> &'static str {
            self.query_debug_name
        }

        /// Returns the debug name of the ingredient.
        pub fn result_debug_name(&self) -> &'static str {
            self.result_debug_name
        }

        /// Returns the total stack size of the fields of any instances of this ingredient, in bytes.
        pub fn size_of_fields(&self) -> usize {
            self.size_of_fields
        }

        /// Returns the total heap size of the fields of any instances of this ingredient, in bytes.
        ///
        /// Returns `None` if the ingredient doesn't specify a `heap_size` function.
        pub fn heap_size_of_fields(&self) -> Option<usize> {
            self.heap_size_of_fields
        }

        /// Returns the total size of Salsa metadata of any instances of this ingredient, in bytes.
        pub fn size_of_metadata(&self) -> usize {
            self.size_of_metadata
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    pub struct StructMemoryInfo {
        pub(crate) debug_name: &'static str,
        pub(crate) size_of_metadata: usize,
        pub(crate) size_of_fields: usize,
        pub(crate) heap_size_of_fields: Option<usize>,
    }

    impl StructMemoryInfo {
        /// Returns the debug name of the ingredient.
        pub fn debug_name(&self) -> &'static str {
            self.debug_name
        }

        /// Returns the total stack size of the fields of any instances of this ingredient, in bytes.
        pub fn size_of_fields(&self) -> usize {
            self.size_of_fields
        }

        /// Returns the total heap size of the fields of any instances of this ingredient, in bytes.
        ///
        /// Returns `None` if the ingredient doesn't specify a `heap_size` function.
        pub fn heap_size_of_fields(&self) -> Option<usize> {
            self.heap_size_of_fields
        }

        /// Returns the total size of Salsa metadata of any instances of this ingredient, in bytes.
        pub fn size_of_metadata(&self) -> usize {
            self.size_of_metadata
        }
    }
}
