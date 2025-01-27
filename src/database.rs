use std::borrow::Cow;
use std::ptr::NonNull;

use crate::views::DatabaseDownCaster;
use crate::zalsa::{IngredientIndex, ZalsaDatabase};
use crate::{Durability, Revision};

#[derive(Copy, Clone)]
pub struct RawDatabase<'db> {
    pub(crate) ptr: NonNull<()>,
    _marker: std::marker::PhantomData<&'db dyn Database>,
}

impl<'db, Db: Database + ?Sized> From<&'db Db> for RawDatabase<'db> {
    #[inline]
    fn from(db: &'db Db) -> Self {
        RawDatabase {
            ptr: NonNull::from(db).cast(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'db, Db: Database + ?Sized> From<&'db mut Db> for RawDatabase<'db> {
    #[inline]
    fn from(db: &'db mut Db) -> Self {
        RawDatabase {
            ptr: NonNull::from(db).cast(),
            _marker: std::marker::PhantomData,
        }
    }
}

/// The trait implemented by all Salsa databases.
/// You can create your own subtraits of this trait using the `#[salsa::db]`(`crate::db`) procedural macro.
pub trait Database: Send + ZalsaDatabase + AsDynDatabase {
    /// Enforces current LRU limits, evicting entries if necessary.
    ///
    /// **WARNING:** Just like an ordinary write, this method triggers
    /// cancellation. If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn trigger_lru_eviction(&mut self) {
        let zalsa_mut = self.zalsa_mut();
        zalsa_mut.reset_for_new_revision();
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
    fn zalsa_register_downcaster(&self) -> &DatabaseDownCaster<dyn Database> {
        self.zalsa().views().downcaster_for::<dyn Database>()
        // The no-op downcaster is special cased in view caster construction.
    }

    #[doc(hidden)]
    #[inline(always)]
    fn downcast(&self) -> &dyn Database
    where
        Self: Sized,
    {
        // No-op
        self
    }
}

/// Upcast to a `dyn Database`.
///
/// Only required because upcasting does not work for unsized generic parameters.
pub trait AsDynDatabase {
    fn as_dyn_database(&self) -> &dyn Database;
}

impl<T: Database> AsDynDatabase for T {
    #[inline(always)]
    fn as_dyn_database(&self) -> &dyn Database {
        self
    }
}

pub fn current_revision<Db: ?Sized + Database>(db: &Db) -> Revision {
    db.zalsa().current_revision()
}

#[cfg(feature = "salsa_unstable")]
pub use memory_usage::IngredientInfo;

#[cfg(feature = "salsa_unstable")]
pub(crate) use memory_usage::{MemoInfo, SlotInfo};

#[cfg(feature = "salsa_unstable")]
mod memory_usage {
    use crate::Database;
    use hashbrown::HashMap;

    impl dyn Database {
        /// Returns information about any Salsa structs.
        pub fn structs_info(&self) -> Vec<IngredientInfo> {
            self.zalsa()
                .ingredients()
                .filter_map(|ingredient| {
                    let mut size_of_fields = 0;
                    let mut size_of_metadata = 0;
                    let mut instances = 0;

                    for slot in ingredient.memory_usage(self)? {
                        instances += 1;
                        size_of_fields += slot.size_of_fields;
                        size_of_metadata += slot.size_of_metadata;
                    }

                    Some(IngredientInfo {
                        count: instances,
                        size_of_fields,
                        size_of_metadata,
                        debug_name: ingredient.debug_name(),
                    })
                })
                .collect()
        }

        /// Returns information about any memoized Salsa queries.
        ///
        /// The returned map holds memory usage information for memoized values of a given query, keyed
        /// by the query function name.
        pub fn queries_info(&self) -> HashMap<&'static str, IngredientInfo> {
            let mut queries = HashMap::new();

            for input_ingredient in self.zalsa().ingredients() {
                let Some(input_info) = input_ingredient.memory_usage(self) else {
                    continue;
                };

                for input in input_info {
                    for memo in input.memos {
                        let info = queries.entry(memo.debug_name).or_insert(IngredientInfo {
                            debug_name: memo.output.debug_name,
                            ..Default::default()
                        });

                        info.count += 1;
                        info.size_of_fields += memo.output.size_of_fields;
                        info.size_of_metadata += memo.output.size_of_metadata;
                    }
                }
            }

            queries
        }
    }

    /// Information about instances of a particular Salsa ingredient.
    #[derive(Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct IngredientInfo {
        debug_name: &'static str,
        count: usize,
        size_of_metadata: usize,
        size_of_fields: usize,
    }

    impl IngredientInfo {
        /// Returns the debug name of the ingredient.
        pub fn debug_name(&self) -> &'static str {
            self.debug_name
        }

        /// Returns the total size of the fields of any instances of this ingredient, in bytes.
        pub fn size_of_fields(&self) -> usize {
            self.size_of_fields
        }

        /// Returns the total size of Salsa metadata of any instances of this ingredient, in bytes.
        pub fn size_of_metadata(&self) -> usize {
            self.size_of_metadata
        }

        /// Returns the number of instances of this ingredient.
        pub fn count(&self) -> usize {
            self.count
        }
    }

    /// Memory usage information about a particular instance of struct, input or output.
    pub struct SlotInfo {
        pub(crate) debug_name: &'static str,
        pub(crate) size_of_metadata: usize,
        pub(crate) size_of_fields: usize,
        pub(crate) memos: Vec<MemoInfo>,
    }

    /// Memory usage information about a particular memo.
    pub struct MemoInfo {
        pub(crate) debug_name: &'static str,
        pub(crate) output: SlotInfo,
    }
}
