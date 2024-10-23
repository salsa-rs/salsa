use crate::durability::Durability;
use crate::id::AsId;
use crate::ingredient::fmt_index;
use crate::key::DependencyIndex;
use crate::plumbing::{Jar, JarAux};
use crate::table::memo::MemoTable;
use crate::table::sync::SyncTable;
use crate::table::Slot;
use crate::zalsa::IngredientIndex;
use crate::zalsa_local::QueryOrigin;
use crate::{Database, DatabaseKeyIndex, Id};
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use super::hash::FxDashMap;
use super::ingredient::Ingredient;
use super::Revision;

pub trait Configuration: Sized + 'static {
    const DEBUG_NAME: &'static str;

    /// The type of data being interned
    type Data<'db>: InternedData;

    /// The end user struct
    type Struct<'db>: Copy;

    /// Create an end-user struct from the salsa id
    ///
    /// This call is an "end-step" to the tracked struct lookup/creation
    /// process in a given revision: it occurs only when the struct is newly
    /// created or, if a struct is being reused, after we have updated its
    /// fields (or confirmed it is green and no updates are required).
    fn struct_from_id<'db>(id: Id) -> Self::Struct<'db>;

    /// Deref the struct to yield the underlying id.
    fn deref_struct(s: Self::Struct<'_>) -> Id;
}

pub trait InternedData: Sized + Eq + Hash + Clone + Sync + Send {}
impl<T: Eq + Hash + Clone + Sync + Send> InternedData for T {}

pub struct JarImpl<C: Configuration> {
    phantom: PhantomData<C>,
}

/// The interned ingredient hashes values of type `Data` to produce an `Id`.
///
/// It used to store interned structs but also to store the id fields of a tracked struct.
/// Interned values endure until they are explicitly removed in some way.
pub struct IngredientImpl<C: Configuration> {
    /// Index of this ingredient in the database (used to construct database-ids, etc).
    ingredient_index: IngredientIndex,

    /// Maps from data to the existing interned id for that data.
    ///
    /// Deadlock requirement: We access `value_map` while holding lock on `key_map`, but not vice versa.
    key_map: FxDashMap<C::Data<'static>, Id>,

    /// Stores the revision when this interned ingredient was last cleared.
    /// You can clear an interned table at any point, deleting all its entries,
    /// but that will make anything dependent on those entries dirty and in need
    /// of being recomputed.
    reset_at: Revision,
}

/// Struct storing the interned fields.
pub struct Value<C>
where
    C: Configuration,
{
    data: C::Data<'static>,
    memos: MemoTable,
    syncs: SyncTable,
}

impl<C: Configuration> Default for JarImpl<C> {
    fn default() -> Self {
        Self {
            phantom: PhantomData,
        }
    }
}

impl<C: Configuration> Jar for JarImpl<C> {
    fn create_ingredients(
        &self,
        _aux: &dyn JarAux,
        first_index: IngredientIndex,
    ) -> Vec<Box<dyn Ingredient>> {
        vec![Box::new(IngredientImpl::<C>::new(first_index)) as _]
    }
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn new(ingredient_index: IngredientIndex) -> Self {
        Self {
            ingredient_index,
            key_map: Default::default(),
            reset_at: Revision::start(),
        }
    }

    unsafe fn to_internal_data<'db>(&'db self, data: C::Data<'db>) -> C::Data<'static> {
        unsafe { std::mem::transmute(data) }
    }

    unsafe fn from_internal_data<'db>(data: &'db C::Data<'static>) -> &'db C::Data<'db> {
        unsafe { std::mem::transmute(data) }
    }

    pub fn intern_id<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        data: impl Lookup<C::Data<'db>>,
    ) -> crate::Id {
        C::deref_struct(self.intern(db, data)).as_id()
    }

    /// Intern data to a unique reference.
    pub fn intern<'db>(
        &'db self,
        db: &'db dyn crate::Database,
        data: impl Lookup<C::Data<'db>>,
    ) -> C::Struct<'db> {
        let zalsa_local = db.zalsa_local();
        zalsa_local.report_tracked_read(
            DependencyIndex::for_table(self.ingredient_index),
            Durability::MAX,
            self.reset_at,
            &Default::default(),
        );

        // Optimisation to only get read lock on the map if the data has already
        // been interned.
        // We need to use the raw API for this lookup. See the [`Lookup`][] trait definition for an explanation of why.
        let data_hash = {
            let mut hasher = self.key_map.hasher().build_hasher();
            data.hash(&mut hasher);
            hasher.finish()
        };
        let shard = self.key_map.determine_shard(data_hash as _);
        {
            let lock = self.key_map.shards()[shard].read();
            if let Some(bucket) = lock.find(data_hash, |(a, _)| {
                // SAFETY: it's safe to go from Data<'static> to Data<'db>
                // shrink lifetime here to use a single lifetime in Lookup::eq(&StructKey<'db>, &C::Data<'db>)
                let a: &C::Data<'db> = unsafe { std::mem::transmute(a) };
                Lookup::eq(&data, a)
            }) {
                // SAFETY: Read lock on map is held during this block
                return C::struct_from_id(unsafe { *bucket.as_ref().1.get() });
            }
        };

        let data = data.into_owned();

        let internal_data = unsafe { self.to_internal_data(data) };

        match self.key_map.entry(internal_data.clone()) {
            // Data has been interned by a racing call, use that ID instead
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                let id = *entry.get();
                drop(entry);
                C::struct_from_id(id)
            }

            // We won any races so should intern the data
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let zalsa = db.zalsa();
                let table = zalsa.table();
                let next_id = zalsa_local.allocate(table, self.ingredient_index, || Value::<C> {
                    data: internal_data,
                    memos: Default::default(),
                    syncs: Default::default(),
                });
                entry.insert(next_id);
                C::struct_from_id(next_id)
            }
        }
    }

    /// Lookup the data for an interned value based on its id.
    /// Rarely used since end-users generally carry a struct with a pointer directly
    /// to the interned item.
    pub fn data<'db>(&'db self, db: &'db dyn Database, id: Id) -> &'db C::Data<'db> {
        let internal_data = db.zalsa().table().get::<Value<C>>(id);
        unsafe { Self::from_internal_data(&internal_data.data) }
    }

    /// Lookup the fields from an interned struct.
    /// Note that this is not "leaking" since no dependency edge is required.
    pub fn fields<'db>(&'db self, db: &'db dyn Database, s: C::Struct<'db>) -> &'db C::Data<'db> {
        self.data(db, C::deref_struct(s))
    }

    pub fn reset(&mut self, revision: Revision) {
        assert!(revision > self.reset_at);
        self.reset_at = revision;
        self.key_map.clear();
    }
}

impl<C> Ingredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.ingredient_index
    }

    fn maybe_changed_after(
        &self,
        _db: &dyn Database,
        _input: Option<Id>,
        revision: Revision,
    ) -> bool {
        revision < self.reset_at
    }

    fn cycle_recovery_strategy(&self) -> crate::cycle::CycleRecoveryStrategy {
        crate::cycle::CycleRecoveryStrategy::Panic
    }

    fn origin(&self, _db: &dyn Database, _key_index: crate::Id) -> Option<QueryOrigin> {
        None
    }

    fn mark_validated_output(
        &self,
        _db: &dyn Database,
        executor: DatabaseKeyIndex,
        output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "mark_validated_output({:?}, {:?}): input cannot be the output of a tracked function",
            executor, output_key
        );
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
        executor: DatabaseKeyIndex,
        stale_output_key: Option<crate::Id>,
    ) {
        unreachable!(
            "remove_stale_output({:?}, {:?}): interned ids are not outputs",
            executor, stale_output_key
        );
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        false
    }

    fn reset_for_new_revision(&mut self) {
        // Interned ingredients do not, normally, get deleted except when they are "reset" en masse.
        // There ARE methods (e.g., `clear_deleted_entries` and `remove`) for deleting individual
        // items, but those are only used for tracked struct ingredients.
        panic!("unexpected call to `reset_for_new_revision`")
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(C::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
    }

    fn accumulated<'db>(
        &'db self,
        _db: &'db dyn Database,
        _key_index: Id,
    ) -> Option<&'db crate::accumulator::accumulated_map::AccumulatedMap> {
        None
    }
}

impl<C> std::fmt::Debug for IngredientImpl<C>
where
    C: Configuration,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.ingredient_index)
            .finish()
    }
}

impl<C> Slot for Value<C>
where
    C: Configuration,
{
    unsafe fn memos(&self, _current_revision: Revision) -> &MemoTable {
        &self.memos
    }

    unsafe fn syncs(&self, _current_revision: Revision) -> &crate::table::sync::SyncTable {
        &self.syncs
    }
}

/// The `Lookup` trait is a more flexible variant on [`std::borrow::Borrow`]
/// and [`std::borrow::ToOwned`].
///
/// It is implemented by "some type that can be used as the lookup key for `O`".
/// This means that `self` can be hashed and compared for equality with values
/// of type `O` without actually creating an owned value. It `self` needs to be interned,
/// it can be converted into an equivalent value of type `O`.
///
/// The canonical example is `&str: Lookup<String>`. However, this example
/// alone can be handled by [`std::borrow::Borrow`][]. In our case, we may have
/// multiple keys accumulated into a struct, like `ViewStruct: Lookup<(K1, ...)>`,
/// where `struct ViewStruct<L1: Lookup<K1>...>(K1...)`. The `Borrow` trait
/// requires that `&(K1...)` be convertible to `&ViewStruct` which just isn't
/// possible. `Lookup` instead offers direct `hash` and `eq` methods.
pub trait Lookup<O> {
    fn hash<H: Hasher>(&self, h: &mut H);
    fn eq(&self, data: &O) -> bool;
    fn into_owned(self) -> O;
}

impl<T> Lookup<T> for T
where
    T: Hash + Eq,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h);
    }

    fn eq(&self, data: &T) -> bool {
        self == data
    }

    fn into_owned(self) -> T {
        self
    }
}

impl<T> Lookup<T> for &T
where
    T: Clone + Eq + Hash,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h);
    }

    fn eq(&self, data: &T) -> bool {
        *self == data
    }

    fn into_owned(self) -> T {
        Clone::clone(self)
    }
}

impl<'a, T> Lookup<Box<T>> for &'a T
where
    T: ?Sized + Hash + Eq,
    Box<T>: From<&'a T>,
{
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h)
    }
    fn eq(&self, data: &Box<T>) -> bool {
        **self == **data
    }
    fn into_owned(self) -> Box<T> {
        Box::from(self)
    }
}

impl Lookup<String> for &str {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, &mut *h)
    }

    fn eq(&self, data: &String) -> bool {
        self == data
    }

    fn into_owned(self) -> String {
        self.to_owned()
    }
}

impl<A: Hash + Eq + PartialEq<T> + Clone + Lookup<T>, T> Lookup<Vec<T>> for &[A] {
    fn hash<H: Hasher>(&self, h: &mut H) {
        for a in *self {
            Hash::hash(a, h);
        }
    }

    fn eq(&self, data: &Vec<T>) -> bool {
        self.len() == data.len() && data.iter().enumerate().all(|(i, a)| &self[i] == a)
    }

    fn into_owned(self) -> Vec<T> {
        self.iter().map(|a| Lookup::into_owned(a.clone())).collect()
    }
}

impl<const N: usize, A: Hash + Eq + PartialEq<T> + Clone + Lookup<T>, T> Lookup<Vec<T>> for [A; N] {
    fn hash<H: Hasher>(&self, h: &mut H) {
        for a in self {
            Hash::hash(a, h);
        }
    }

    fn eq(&self, data: &Vec<T>) -> bool {
        self.len() == data.len() && data.iter().enumerate().all(|(i, a)| &self[i] == a)
    }

    fn into_owned(self) -> Vec<T> {
        self.into_iter()
            .map(|a| Lookup::into_owned(a.clone()))
            .collect()
    }
}

impl Lookup<PathBuf> for &Path {
    fn hash<H: Hasher>(&self, h: &mut H) {
        Hash::hash(self, h);
    }

    fn eq(&self, data: &PathBuf) -> bool {
        self == data
    }

    fn into_owned(self) -> PathBuf {
        self.to_owned()
    }
}
