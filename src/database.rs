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

    /// This method triggers cancellation.
    /// If you invoke it while a snapshot exists, it
    /// will block until that snapshot is dropped -- if that snapshot
    /// is owned by the current thread, this could trigger deadlock.
    fn trigger_cancellation(&mut self) {
        let _ = self.zalsa_mut();
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

#[cfg(feature = "persistence")]
pub use persistence::SerializeBuilder;

#[cfg(feature = "persistence")]
pub(crate) mod persistence {
    use crate::plumbing::{Ingredient, Persistable};
    use crate::zalsa::{HasJar, Zalsa};
    use crate::{Database, IngredientIndex};

    use std::fmt;

    use serde::de::{self, DeserializeSeed, SeqAccess};

    impl dyn Database {
        /// Returns a type implementing [`serde::Serialize`], that can be used to serialize the
        /// current state of the database.
        pub fn as_serialize<'db, I>(
            &'db mut self,
            ingredients: impl Fn(SerializeBuilder<'db>) -> I,
        ) -> impl serde::Serialize + '_
        where
            I: serde::Serialize + 'db,
        {
            (self.zalsa().runtime(), ingredients(SerializeBuilder(self)))
        }

        /// Deserialize the database using a [`serde::Deserializer`].
        ///
        /// This method will modify the database in-place based on the serialized data.
        pub fn deserialize<'db, D>(&mut self, deserializer: D) -> Result<(), D::Error>
        where
            D: serde::Deserializer<'db>,
        {
            DeserializeDatabase(self.zalsa_mut()).deserialize(deserializer)
        }
    }
   
    // TODO: ingredient_set! { Interned<'_>, Tracked<'_>, query, ... }
    // TODO: macro_rules! ingredient_set

    pub struct SerializeBuilder<'db>(&'db dyn Database);

    impl<'db> SerializeBuilder<'db> {
        pub fn ingredient<I>(&self) -> impl serde::Serialize + 'db
        where
            I: HasJar,
            I::Ingredient: Persistable,
        {
            // SAFETY: `Database::as_serialize` captures a mutable reference to the database.
            unsafe { I::ingredient(self.0).as_serialize(self.0.zalsa()) }
        }
    }

    pub struct DeserializeBuilder<'db> {
        db: &'db dyn Database,
        ingredients: Vec<fn()>
    }

    impl<'db> SerializeBuilder<'db> {
        pub fn ingredient<I>(&self) -> impl serde::Serialize + 'db
        where
            I: HasJar,
            I::Ingredient: Persistable,
        {
            // SAFETY: `Database::as_serialize` captures a mutable reference to the database.
            unsafe { I::ingredient(self.0).as_serialize(self.0.zalsa()) }
        }
    }

    /// Upgrades a shared reference to an ingredient into a mutable reference given a mutable
    /// reference to the database.
    ///
    /// This method will temporarily remove the ingredient from the database, so any attempts
    /// to lookup the ingredient will fail.
    pub(crate) fn with_mut_ingredient<I, R>(
        ingredient: &I,
        zalsa: &mut Zalsa,
        f: impl FnOnce(&mut I, &mut Zalsa) -> R,
    ) -> R
    where
        I: Ingredient,
    {
        let index = ingredient.ingredient_index();

        // Remove the ingredient temporarily, to avoid holding overlapping mutable borrows.
        let mut ingredient = zalsa.take_ingredient(index);

        // Call the function with the concrete ingredient.
        let ingredient_mut = <dyn std::any::Any>::downcast_mut(&mut *ingredient).unwrap();
        let value = f(ingredient_mut, zalsa);

        zalsa.replace_ingredient(index, ingredient);
        value
    }

    #[derive(serde::Deserialize)]
    #[serde(field_identifier, rename_all = "lowercase")]
    enum DatabaseField {
        Runtime,
        Ingredients,
    }

    struct DeserializeDatabase<'db>(&'db mut Zalsa);

    impl<'de> de::DeserializeSeed<'de> for DeserializeDatabase<'_> {
        type Value = ();

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: de::Deserializer<'de>,
        {
            deserializer.deserialize_seq(self)
        }
    }

    impl<'de> serde::de::Visitor<'de> for DeserializeDatabase<'_> {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("struct Database")
        }

        fn visit_seq<V>(self, mut seq: V) -> Result<(), V::Error>
        where
            V: SeqAccess<'de>,
        {
            let mut runtime = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(0, &self))?;
            let () = seq
                .next_element_seed(DeserializeIngredients(self.0))?
                .ok_or_else(|| de::Error::invalid_length(1, &self))?;

            self.0.runtime_mut().deserialize_from(&mut runtime);
            Ok(())
        }
    }

    struct DeserializeIngredients<'db>(&'db mut Zalsa);

    impl<'de> serde::de::DeserializeSeed<'de> for DeserializeIngredients<'_> {
        type Value = ();

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_seq(self)
        }
    }

    impl<'de> serde::de::Visitor<'de> for DeserializeIngredients<'_> {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            while let Some(x) = seq.next_element_seed(seed) {}
        }
    }
}

#[cfg(feature = "salsa_unstable")]
pub use memory_usage::IngredientInfo;

#[cfg(feature = "salsa_unstable")]
pub(crate) use memory_usage::{MemoInfo, SlotInfo};

#[cfg(feature = "salsa_unstable")]
mod memory_usage {
    use hashbrown::HashMap;

    use crate::Database;

    impl dyn Database {
        /// Returns memory usage information about ingredients in the database.
        pub fn memory_usage(&self) -> DatabaseInfo {
            let mut queries = HashMap::new();
            let mut structs = Vec::new();

            for input_ingredient in self.zalsa().ingredients() {
                let Some(input_info) = input_ingredient.memory_usage(self) else {
                    continue;
                };

                let mut size_of_fields = 0;
                let mut size_of_metadata = 0;
                let mut count = 0;
                let mut heap_size_of_fields = None;

                for input_slot in input_info {
                    count += 1;
                    size_of_fields += input_slot.size_of_fields;
                    size_of_metadata += input_slot.size_of_metadata;

                    if let Some(slot_heap_size) = input_slot.heap_size_of_fields {
                        heap_size_of_fields =
                            Some(heap_size_of_fields.unwrap_or_default() + slot_heap_size);
                    }

                    for memo in input_slot.memos {
                        let info = queries.entry(memo.debug_name).or_insert(IngredientInfo {
                            debug_name: memo.output.debug_name,
                            ..Default::default()
                        });

                        info.count += 1;
                        info.size_of_fields += memo.output.size_of_fields;
                        info.size_of_metadata += memo.output.size_of_metadata;

                        if let Some(memo_heap_size) = memo.output.heap_size_of_fields {
                            info.heap_size_of_fields =
                                Some(info.heap_size_of_fields.unwrap_or_default() + memo_heap_size);
                        }
                    }
                }

                structs.push(IngredientInfo {
                    count,
                    size_of_fields,
                    size_of_metadata,
                    heap_size_of_fields,
                    debug_name: input_ingredient.debug_name(),
                });
            }

            DatabaseInfo { structs, queries }
        }
    }

    /// Memory usage information about ingredients in the Salsa database.
    pub struct DatabaseInfo {
        /// Information about any Salsa structs.
        pub structs: Vec<IngredientInfo>,

        /// Memory usage information for memoized values of a given query, keyed
        /// by the query function name.
        pub queries: HashMap<&'static str, IngredientInfo>,
    }

    /// Information about instances of a particular Salsa ingredient.
    #[derive(Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct IngredientInfo {
        debug_name: &'static str,
        count: usize,
        size_of_metadata: usize,
        size_of_fields: usize,
        heap_size_of_fields: Option<usize>,
    }

    impl IngredientInfo {
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
        pub(crate) heap_size_of_fields: Option<usize>,
        pub(crate) memos: Vec<MemoInfo>,
    }

    /// Memory usage information about a particular memo.
    pub struct MemoInfo {
        pub(crate) debug_name: &'static str,
        pub(crate) output: SlotInfo,
    }
}
