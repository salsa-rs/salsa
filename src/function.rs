pub(crate) use maybe_changed_after::VerifyResult;
pub(crate) use memo::ErasedMemo;
pub(crate) use sync::{ClaimGuard, ClaimResult, Reentrancy, SyncGuard, SyncOwner, SyncTable};

use std::any::Any;
use std::fmt;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use crate::cycle::{CycleRecoveryStrategy, IterationStamp, ProvisionalStatus};
use crate::database::RawDatabase;
use crate::function::delete::DeletedEntries;
use crate::hash::{FxHashSet, FxIndexSet};
use crate::ingredient::Ingredient;
use crate::key::DatabaseKeyIndex;
use crate::plumbing::{self, MemoIngredientMap};
use crate::salsa_struct::SalsaStructInDb;
use crate::sync::Arc;
use crate::table::Table;
use crate::table::memo::{MemoSlot, MemoTableTypes};
use crate::views::DatabaseDownCaster;
use crate::zalsa::{IngredientIndex, JarKind, MemoIngredientIndex, Zalsa};
use crate::zalsa_local::QueryEdge;
use crate::{Cycle, Id, Revision};

#[cfg(feature = "accumulator")]
mod accumulated;
mod backdate;
mod cached_value;
mod delete;
mod diff_outputs;
mod eviction;
mod execute;
mod fetch;
pub use cached_value::CachedValue;
#[cfg(feature = "persistence")]
pub use cached_value::LazyValue;
mod maybe_changed_after;
mod memo;
mod specify;
mod sync;

pub use eviction::{EvictionPolicy, HasCapacity, Lru, NoopEviction};

pub type Memo<C> = memo::Memo<C>;

/// Storage for a fingerprint in a value-less memo. An uninhabited fingerprint
/// keeps ordinary memos the same size as an `Option<Output>` plus the header.
#[doc(hidden)]
pub trait StoredFingerprint: Copy + std::fmt::Debug + Send + Sync + 'static {
    fn from_bytes(bytes: [u8; 16]) -> Self;
    fn bytes(self) -> [u8; 16];
}

impl StoredFingerprint for [u8; 16] {
    fn from_bytes(bytes: [u8; 16]) -> Self {
        bytes
    }
    fn bytes(self) -> [u8; 16] {
        self
    }
}

impl StoredFingerprint for std::convert::Infallible {
    fn from_bytes(_: [u8; 16]) -> Self {
        unreachable!("this query persists no fingerprint")
    }
    fn bytes(self) -> [u8; 16] {
        match self {}
    }
}

/// Configuration for a Salsa function ingredient.
///
/// # Safety
///
/// For every lifetime `'db`, `Output<'db>` must be safe for Salsa to retain
/// after erasing `'db` and to use after rebranding it with a later database
/// lifetime. This is guaranteed when the output implements [`crate::SalsaValue`]
/// or when it is the same `'static` type for every `'db`.
pub unsafe trait Configuration: Any {
    const DEBUG_NAME: &'static str;
    const LOCATION: crate::ingredient::Location;
    const PERSIST: bool;

    type StoredFingerprint: StoredFingerprint;
    type CachedValue: CachedValue<Self>;

    /// Stable, process-independent fingerprint used in place of a persisted output.
    const FINGERPRINT: Option<for<'db> fn(&Self::Output<'db>) -> u128> = None;

    /// The database that this function is associated with.
    type DbView: ?Sized + crate::Database;

    /// The "salsa struct type" that this function is associated with.
    /// This can be just `salsa::Id` for functions that intern their arguments
    /// and are not clearly associated with any one salsa struct.
    type SalsaStruct<'db>: SalsaStructInDb;

    /// The input to the function
    type Input<'db>: Send + Sync;

    /// The value computed by the function.
    type Output<'db>: Send + Sync;

    /// The eviction policy for this function's memoized values.
    type Eviction: EvictionPolicy;

    /// Determines whether this function can recover from being a participant in a cycle
    /// (and, if so, how).
    const CYCLE_STRATEGY: CycleRecoveryStrategy;

    /// Invokes after a new result `new_value` has been computed for which an older memoized value
    /// existed `old_value`, or in fixpoint iteration. Returns true if the new value is equal to
    /// the older one.
    ///
    /// This invokes user code in form of the `Eq` impl.
    fn values_equal<'db>(old_value: &Self::Output<'db>, new_value: &Self::Output<'db>) -> bool;

    /// Convert from the id used internally to the value that execute is expecting.
    /// This is a no-op if the input to the function is a salsa struct.
    fn id_to_input(zalsa: &Zalsa, key: Id) -> Self::Input<'_>;

    /// Returns the size of any heap allocations in the output value, in bytes.
    fn heap_size(_value: &Self::Output<'_>) -> Option<usize> {
        None
    }

    /// Invoked when we need to compute the value for the given key, either because we've never
    /// computed it before or because the old one relied on inputs that have changed.
    ///
    /// This invokes the function the user wrote.
    fn execute<'db>(db: &'db Self::DbView, input: Self::Input<'db>) -> Self::Output<'db>;

    /// Get the cycle recovery initial value.
    ///
    /// `last_provisional_value` is `Some` when this query was a participant in a broader
    /// cycle and becomes a new cycle head. It contains the query's previous provisional
    /// result. Otherwise, it is `None`.
    fn cycle_initial<'db>(
        db: &'db Self::DbView,
        id: Id,
        last_provisional_value: Option<&Self::Output<'db>>,
        input: Self::Input<'db>,
    ) -> Self::Output<'db>;

    /// Decide what value to use for this cycle iteration. Takes ownership of the new value
    /// and returns an owned value to use.
    ///
    /// The function is called for every iteration of the cycle head, regardless of whether the cycle
    /// has converged (the values are equal).
    ///
    /// # Id
    ///
    /// The id can be used to uniquely identify the query instance. This can be helpful
    /// if the cycle function has to re-identify a value it returned previously.
    ///
    /// # Values
    ///
    /// The `last_provisional_value` is the value from the previous iteration of this cycle
    /// and `value` is the new value that was computed in the current iteration.
    ///
    /// # Iteration count
    ///
    /// The `iteration` parameter isn't guaranteed to start from zero or to be contiguous:
    ///
    /// * **Initial value**: `iteration` may be non-zero on the first call for a given query if that
    ///   query becomes the outermost cycle head after a nested cycle complete a few iterations. In this case,
    ///   `iteration` continues from the nested cycle's iteration count rather than resetting to zero.
    /// * **Non-contiguous values**: The iteration count can be non-contigious for cycle heads
    ///   that are only conditionally part of a cycle.
    ///
    /// # Return value
    ///
    /// The function should return the value to use for this iteration. This can be the `value`
    /// that was computed, or a different value (e.g., a fallback value). This cycle will continue
    /// iterating until the returned value equals the previous iteration's value.
    fn recover_from_cycle<'db>(
        db: &'db Self::DbView,
        cycle: &Cycle,
        last_provisional_value: &Self::Output<'db>,
        value: Self::Output<'db>,
        input: Self::Input<'db>,
    ) -> Self::Output<'db>;

    /// Serialize the output type using `serde`.
    ///
    /// Panics if the value is not persistable, i.e. `Configuration::PERSIST` is `false`.
    fn serialize<S>(value: &Self::Output<'_>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: plumbing::serde::Serializer;

    /// Deserialize the output type using `serde`.
    ///
    /// Panics if the value is not persistable, i.e. `Configuration::PERSIST` is `false`.
    fn deserialize<'de, D>(deserializer: D) -> Result<Self::Output<'static>, D::Error>
    where
        D: plumbing::serde::Deserializer<'de>;
}

/// The function-specific interface for an [`Ingredient`].
///
/// This type is public because it appears in the public [`Ingredient`] trait, but it can only be
/// constructed and used inside salsa.
#[doc(hidden)]
pub struct FunctionIngredientRef<'db> {
    ingredient: &'db dyn FunctionIngredient,
}

impl<'db> FunctionIngredientRef<'db> {
    fn new(ingredient: &'db dyn FunctionIngredient) -> Self {
        Self { ingredient }
    }

    pub(crate) fn memo(&self, zalsa: &'db Zalsa, input: Id) -> Option<ErasedMemo<'db>> {
        self.ingredient.memo(zalsa, input)
    }

    pub(crate) fn sync_table(&self) -> &'db SyncTable {
        self.ingredient.sync_table()
    }

    /// Returns information about the current provisional status of `input`.
    ///
    /// Is it a provisional value, a poisoned provisional memo, or has it been finalized and in
    /// which iteration.
    ///
    /// Returns `None` if `input` doesn't exist.
    pub(crate) fn provisional_status(
        &self,
        zalsa: &'db Zalsa,
        input: Id,
    ) -> Option<ProvisionalStatus<'db>> {
        self.ingredient.provisional_status(zalsa, input)
    }
}

pub(crate) trait FunctionIngredient: Send + Sync {
    fn memo<'db>(&'db self, zalsa: &'db Zalsa, input: Id) -> Option<ErasedMemo<'db>>;

    fn sync_table(&self) -> &SyncTable;

    /// Returns information about the current provisional status of `input`.
    ///
    /// Is it a provisional value, a poisoned provisional memo, or has it been finalized and in
    /// which iteration.
    ///
    /// Returns `None` if `input` doesn't exist.
    fn provisional_status<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        input: Id,
    ) -> Option<ProvisionalStatus<'db>>;
}

/// Function ingredients are the "workhorse" of salsa.
///
/// They are used for tracked functions, for the "value" fields of tracked structs, and for the fields of input structs.
/// The function ingredient is fairly complex and so its code is spread across multiple modules, typically one per method.
/// The main entry points are:
///
/// * the `fetch` method, which is invoked when the function is called by the user's code;
///   it will return a memoized value if one exists, or execute the function otherwise.
/// * the `specify` method, which can only be used when the key is an entity created by the active query.
///   It sets the value of the function imperatively, so that when later fetches occur, they'll return this value.
/// * the `store` method, which can only be invoked with an `&mut` reference, and is to set input fields.
pub struct IngredientImpl<C: Configuration> {
    /// The ingredient index we were assigned in the database.
    /// Used to construct `DatabaseKeyIndex` values.
    index: IngredientIndex,

    /// The index for the memo/sync tables
    ///
    /// This may be a [`crate::memo_ingredient_indices::MemoIngredientSingletonIndex`] or a
    /// [`crate::memo_ingredient_indices::MemoIngredientIndices`], depending on whether the
    /// tracked function's struct is a plain salsa struct or an enum `#[derive(Supertype)]`.
    memo_ingredient_indices: <C::SalsaStruct<'static> as SalsaStructInDb>::MemoIngredientMap,

    /// Eviction policy - type determined by Configuration.
    /// Used to find memos to throw out when we have too many memoized values.
    eviction: C::Eviction,

    /// An downcaster to `C::DbView`.
    ///
    /// # Safety
    ///
    /// The supplied database must be be the same as the database used to construct the [`Views`]
    /// instances that this downcaster was derived from.
    view_caster: OnceLock<DatabaseDownCaster<C::DbView>>,

    sync_table: SyncTable,

    /// When `fetch` and friends execute, they return a reference to the
    /// value stored in the memo. Its lifetime is tied to the `&self`
    /// reference we start with. This means that whenever we remove something
    /// from `memo_map` with an `&self` reference, there *could* be references to its
    /// internals still in use. Therefore we push the memo into this queue and
    /// only *actually* free up memory when we have `&mut self`.
    ///
    /// You might think that we could do this only if the memo was verified in the
    /// current revision: you would be right, but we are being defensive, because
    /// we don't know that we can trust the database to give us the same runtime
    /// everytime and so forth.
    deleted_entries: DeletedEntries<C>,
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn new(
        index: IngredientIndex,
        memo_ingredient_indices: <C::SalsaStruct<'static> as SalsaStructInDb>::MemoIngredientMap,
        eviction_capacity: usize,
    ) -> Self {
        Self {
            index,
            memo_ingredient_indices,
            eviction: C::Eviction::new(eviction_capacity),
            deleted_entries: Default::default(),
            view_caster: OnceLock::new(),
            sync_table: SyncTable::new(index),
        }
    }

    /// Set the view-caster for this tracked function ingredient, if it has
    /// not already been initialized.
    #[inline]
    pub fn get_or_init(
        &self,
        view_caster: impl FnOnce() -> DatabaseDownCaster<C::DbView>,
    ) -> &Self {
        // Note that we must set this lazily as we don't have access to the database
        // type when ingredients are registered into the `Zalsa`.
        self.view_caster.get_or_init(view_caster);
        self
    }

    #[inline]
    pub fn database_key_index(&self, key: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.index, key)
    }

    /// Set eviction capacity. Only available when eviction policy supports it.
    pub fn set_capacity(&mut self, capacity: usize)
    where
        C::Eviction: HasCapacity,
    {
        self.eviction.set_capacity(capacity);
    }

    fn insert_memo<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        mut memo: memo::Memo<C>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> &'db memo::Memo<C> {
        #[cfg(feature = "persistence")]
        if let Some(image) = &zalsa.cache_image {
            image.memo_replaced(
                self.database_key_index(id),
                C::PERSIST || !memo.header.revisions.tracked_struct_ids().is_empty(),
                memo.header.origin(),
            );
        }
        if let Some(tracked_struct_ids) = memo.header.revisions.tracked_struct_ids_mut() {
            tracked_struct_ids.shrink_to_fit();
        }

        // We convert to a `NonNull` here as soon as possible because we are going to alias
        // into the `Box`, which is a `noalias` type.
        // FIXME: Use `Box::into_non_null` once stable
        let memo = NonNull::from(Box::leak(Box::new(memo)));

        if let Some(old_value) =
            self.insert_memo_into_table_for(zalsa, id, memo, memo_ingredient_index)
        {
            // In case there is a reference to the old memo out there, we have to store it
            // in the deleted entries. This will get cleared when a new revision starts.
            //
            // SAFETY: Once the revision starts, there will be no outstanding borrows to the
            // memo contents, and so it will be safe to free.
            unsafe { self.deleted_entries.push(old_value) };
        }
        // SAFETY: The table owns the memo until it is replaced. Clearing the displaced
        // memos in `self.deleted_entries` requires exclusive access to the ingredient.
        unsafe { memo.as_ref() }
    }

    #[inline]
    fn memo_ingredient_index(&self, zalsa: &Zalsa, id: Id) -> MemoIngredientIndex {
        self.memo_ingredient_indices.get_zalsa_id(zalsa, id)
    }

    #[cfg(any(not(feature = "persistence"), feature = "accumulator"))]
    fn view_caster(&self) -> &DatabaseDownCaster<C::DbView> {
        self.view_caster
            .get()
            .expect("tracked function ingredients cannot be accessed before calling `init`")
    }
}

impl<C> FunctionIngredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn memo<'db>(&'db self, zalsa: &'db Zalsa, input: Id) -> Option<ErasedMemo<'db>> {
        // SAFETY: `self` is borrowed for `'db`, so `deleted_entries` retains every allocation
        // observed in this slot for that lifetime.
        let memo_slot = unsafe {
            MemoSlot::new(
                zalsa.memo_table_for::<C::SalsaStruct<'_>>(input),
                self.memo_ingredient_index(zalsa, input),
            )
        };

        memo_slot.get_erased()
    }

    fn sync_table(&self) -> &SyncTable {
        &self.sync_table
    }

    /// Returns `final` if the memo has the `verified_final` flag set.
    ///
    /// Otherwise, the value is still provisional or the provisional memo has been poisoned. It
    /// also returns the iteration in which this memo was created (always 0 except for cycle
    /// heads).
    fn provisional_status<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        input: Id,
    ) -> Option<ProvisionalStatus<'db>> {
        let memo =
            self.get_memo_from_table_for(zalsa, input, self.memo_ingredient_index(zalsa, input))?;

        if !memo.has_value() && memo.header.may_be_provisional() {
            return Some(ProvisionalStatus::Poisoned {
                iteration: memo.header.revisions.iteration(),
                verified_at: memo.header.verified_at.load(),
            });
        }

        Some(memo.header.provisional_status())
    }
}

impl<C> Ingredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn location(&self) -> &'static crate::ingredient::Location {
        &C::LOCATION
    }

    fn ingredient_index(&self) -> IngredientIndex {
        self.index
    }

    unsafe fn maybe_changed_after(
        &self,
        _zalsa: &Zalsa,
        db: RawDatabase<'_>,
        input: Id,
        revision: Revision,
    ) -> VerifyResult {
        #[cfg(feature = "persistence")]
        let caster = match self.view_caster.get() {
            Some(caster) => caster,
            None => {
                // A restored dependency can be verified before its accessor runs. If its
                // view is still unknown, executing the caller registers the correct view.
                let Some(caster) = _zalsa.views().try_downcaster_for::<C::DbView>() else {
                    let key = self.database_key_index(input);
                    let slot = self.memo_ingredient_index(_zalsa, input);
                    if let Some(memo) = self.get_memo_from_table_for(_zalsa, input, slot)
                        && let Some(result) = memo.header.maybe_changed_after_hot(
                            _zalsa,
                            key,
                            revision,
                            #[cfg(feature = "detailed-trace")]
                            memo.has_value(),
                        )
                    {
                        return result;
                    }
                    return VerifyResult::changed();
                };
                self.view_caster.get_or_init(|| *caster)
            }
        };
        #[cfg(not(feature = "persistence"))]
        let caster = self.view_caster();
        // SAFETY: The database and the caster both belong to this storage.
        let db = unsafe { caster.downcast_unchecked(db) };
        self.maybe_changed_after(db, input, revision)
    }

    unsafe fn collect_minimum_serialized_edges(
        &self,
        zalsa: &Zalsa,
        edge: QueryEdge,
        serialized_edges: &mut FxIndexSet<QueryEdge>,
        visited_edges: &mut FxHashSet<QueryEdge>,
    ) {
        // SAFETY: The caller prevents database writes throughout the traversal.
        let Some(memo) = (unsafe { self.memo_unchecked(zalsa, edge.key().key_index()) }) else {
            return;
        };

        // SAFETY: The caller prevents database writes throughout the traversal.
        unsafe {
            collect_minimum_serialized_edges(memo, zalsa, edge, serialized_edges, visited_edges);
        }
    }

    fn as_function(&self) -> Option<FunctionIngredientRef<'_>> {
        Some(FunctionIngredientRef::new(self))
    }

    fn flatten_cycle_head_dependencies(
        &self,
        zalsa: &Zalsa,
        id: Id,
        flattened_input_outputs: &mut FxIndexSet<QueryEdge>,
        seen: &mut FxHashSet<DatabaseKeyIndex>,
    ) {
        flatten_cycle_head_dependencies(
            self,
            zalsa,
            self.database_key_index(id),
            C::CYCLE_STRATEGY,
            flattened_input_outputs,
            seen,
        );
    }

    fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        executor: DatabaseKeyIndex,
        output_key: crate::Id,
    ) {
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, output_key);
        let database_key_index = self.database_key_index(output_key);
        let memo_slot = self.memo_slot(zalsa, output_key, memo_ingredient_index);
        specify::validate_specified_value(zalsa, executor, database_key_index, memo_slot);
    }

    fn remove_stale_output(
        &self,
        _zalsa: &Zalsa,
        _executor: DatabaseKeyIndex,
        _stale_output_key: crate::Id,
    ) {
        // This function is invoked when a query Q specifies the value for `stale_output_key` in rev 1,
        // but not in rev 2. We don't do anything in this case, we just leave the (now stale) memo.
        // Since its `verified_at` field has not changed, it will be considered dirty if it is invoked.
    }

    fn requires_reset_for_new_revision(&self) -> bool {
        true
    }

    fn reset_for_new_revision(&mut self, table: &mut Table) {
        self.eviction.for_each_evicted(|evict| {
            let ingredient_index = table.ingredient_index(evict);
            Self::evict_value_from_memo_for(
                table.memos_mut(evict),
                self.memo_ingredient_indices.get(ingredient_index),
            )
        });

        self.deleted_entries.clear();
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
    }

    fn jar_kind(&self) -> JarKind {
        JarKind::TrackedFn
    }

    fn memo_table_types(&self) -> &Arc<MemoTableTypes> {
        unreachable!("function does not allocate pages")
    }

    fn memo_table_types_mut(&mut self) -> &mut Arc<MemoTableTypes> {
        unreachable!("function does not allocate pages")
    }

    #[cfg(feature = "accumulator")]
    unsafe fn accumulated<'db>(
        &'db self,
        db: RawDatabase<'db>,
        key_index: Id,
    ) -> (
        Option<&'db crate::accumulator::accumulated_map::AccumulatedMap>,
        crate::accumulator::accumulated_map::InputAccumulatedValues,
    ) {
        // SAFETY: The `db` belongs to the ingredient as per caller invariant
        let db = unsafe { self.view_caster().downcast_unchecked(db) };
        self.accumulated_map(db, key_index)
    }

    fn is_persistable(&self) -> bool {
        C::PERSIST
    }

    fn should_serialize(&self, zalsa: &Zalsa) -> bool {
        if !C::PERSIST {
            return false;
        }

        // We only serialize the query if there are any memos associated with it.
        for entry in <C::SalsaStruct<'_> as SalsaStructInDb>::entries(zalsa) {
            let memo_ingredient_index = self.memo_ingredient_indices.get(entry.ingredient_index());

            let memo =
                self.get_memo_from_table_for(zalsa, entry.key_index(), memo_ingredient_index);

            if memo.is_some_and(|memo| memo.should_serialize()) {
                return true;
            }
        }

        false
    }

    #[cfg(feature = "persistence")]
    fn supports_cache_pages(&self) -> bool {
        C::PERSIST && C::FINGERPRINT.is_none()
    }

    #[cfg(feature = "persistence")]
    unsafe fn cache_page_stamp(&self, zalsa: &Zalsa, key: crate::DatabaseKeyIndex) -> Option<u128> {
        let memo = self.get_memo_from_table_for(
            zalsa,
            key.key_index(),
            self.memo_ingredient_indices.get(key.ingredient_index()),
        )?;
        (memo.should_serialize() && memo.cached_record().is_some()).then_some(0)
    }

    #[cfg(feature = "persistence")]
    unsafe fn encode_cache(
        &self,
        zalsa: &Zalsa,
        keys: &[crate::DatabaseKeyIndex],
        identities: &crate::persistence::IdentityRows,
    ) -> std::io::Result<Vec<u8>> {
        crate::persistence::encode(&persistence::SerializeIngredient {
            zalsa,
            ingredient: self,
            keys: Some(keys),
            identities: Some(identities),
        })
    }

    #[cfg(feature = "persistence")]
    fn decode_cache_memos(
        &self,
        zalsa: &Zalsa,
        bytes: &bytes::Bytes,
    ) -> std::io::Result<crate::persistence::IdentityRows> {
        let mut identities = crate::persistence::IdentityRows::default();
        crate::persistence::decode_seed(
            bytes,
            persistence::DeserializeIngredient {
                zalsa,
                ingredient: self,
                archive: Some(bytes),
                identities: &mut identities,
            },
        )?;
        Ok(identities)
    }

    #[cfg(feature = "persistence")]
    unsafe fn serialize<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        f: &mut dyn FnMut(&dyn erased_serde::Serialize),
    ) {
        f(&persistence::SerializeIngredient {
            zalsa,
            ingredient: self,
            keys: None,
            identities: None,
        })
    }

    #[cfg(feature = "persistence")]
    fn deserialize(
        &mut self,
        zalsa: &mut Zalsa,
        deserializer: &mut dyn erased_serde::Deserializer,
    ) -> Result<(), erased_serde::Error> {
        let deserialize = persistence::DeserializeIngredient {
            zalsa,
            ingredient: self,
            archive: None,
            identities: &mut crate::persistence::IdentityRows::default(),
        };

        serde::de::DeserializeSeed::deserialize(deserialize, deserializer)
    }
}

/// # Safety
///
/// The caller must prevent database writes throughout the traversal, including query
/// execution that can reuse slots or modify memos. Concurrent read-only access is allowed.
unsafe fn collect_minimum_serialized_edges(
    memo: ErasedMemo<'_>,
    zalsa: &Zalsa,
    edge: QueryEdge,
    serialized_edges: &mut FxIndexSet<QueryEdge>,
    visited_edges: &mut FxHashSet<QueryEdge>,
) {
    visited_edges.insert(edge);

    // Collect the minimum dependency tree.
    for edge in memo.header().origin().edges() {
        // Avoid forming cycles.
        if visited_edges.contains(&edge) {
            continue;
        }

        // Avoid flattening edges that we're going to serialize directly.
        if serialized_edges.contains(&edge) {
            continue;
        }

        let dependency = zalsa.lookup_ingredient(edge.key().ingredient_index());
        // SAFETY: The caller prevents database writes throughout recursive traversal.
        unsafe {
            dependency.collect_minimum_serialized_edges(
                zalsa,
                edge,
                serialized_edges,
                visited_edges,
            );
        }
    }
}

fn flatten_cycle_head_dependencies(
    ingredient: &dyn FunctionIngredient,
    zalsa: &Zalsa,
    database_key_index: DatabaseKeyIndex,
    cycle_recovery_strategy: CycleRecoveryStrategy,
    flattened_input_outputs: &mut FxIndexSet<QueryEdge>,
    seen: &mut FxHashSet<DatabaseKeyIndex>,
) {
    let Some(memo) = ingredient.memo(zalsa, database_key_index.key_index()) else {
        return;
    };

    let memo_header = memo.header();

    // Only flatten dependencies of provisional queries, because only those
    // contain cyclic dependencies.
    if !memo_header.may_be_provisional() {
        flattened_input_outputs.insert(QueryEdge::input(database_key_index));
        return;
    }

    // There's nothing to do if we've visited this query before.
    if !seen.insert(database_key_index) {
        return;
    }

    let inputs = memo_header.origin().inputs();

    match cycle_recovery_strategy {
        // Queries with cycle handling already flattened their own dependencies when completing
        // the query. Cycle participants commonly share most of those inputs, often in long
        // contiguous runs, so compare by ordered index to avoid a hash-table lookup for every
        // edge. On a mismatch, `insert_full` preserves insertion order and resynchronizes the
        // expected index.
        CycleRecoveryStrategy::FallbackImmediate | CycleRecoveryStrategy::Fixpoint => {
            let mut expected_index = 0;
            for input in inputs.map(QueryEdge::input) {
                if flattened_input_outputs.get_index(expected_index) == Some(&input) {
                    expected_index += 1;
                } else {
                    let (index, _) = flattened_input_outputs.insert_full(input);
                    expected_index = index + 1;
                }
            }
        }
        // For regular queries, recurse
        CycleRecoveryStrategy::Panic => {
            for input in inputs {
                let ingredient = zalsa.lookup_ingredient(input.ingredient_index());
                ingredient.flatten_cycle_head_dependencies(
                    zalsa,
                    input.key_index(),
                    flattened_input_outputs,
                    seen,
                );
            }
        }
    }
}

impl memo::MemoHeader {
    fn provisional_status(&self) -> ProvisionalStatus<'_> {
        let iteration = self.revisions.iteration();
        let verified_at = self.verified_at.load();

        if self.revisions.verified_final.load(Ordering::Relaxed) {
            ProvisionalStatus::Final {
                iteration,
                verified_at,
            }
        } else {
            ProvisionalStatus::Provisional {
                iteration,
                verified_at,
                cycle_heads: self.cycle_heads(),
            }
        }
    }

    fn set_cycle_iteration_count(
        &self,
        database_key_index: DatabaseKeyIndex,
        iteration: IterationStamp,
    ) {
        self.revisions
            .set_iteration_count(database_key_index, iteration);
    }

    fn finalize_cycle_head(&self) {
        self.revisions.verified_final.store(true, Ordering::Release);
    }

    fn cycle_converged(&self) -> bool {
        self.revisions.cycle_converged()
    }
}

impl<C> std::fmt::Debug for IngredientImpl<C>
where
    C: Configuration,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("index", &self.index)
            .finish()
    }
}

#[cfg(feature = "persistence")]
mod persistence {
    use super::memo::persistence::{BinaryMemo, BinaryMemoSeed};
    use super::{Configuration, IngredientImpl, Memo};
    use crate::hash::{FxHashSet, FxIndexSet};
    use crate::plumbing::{MemoIngredientMap, SalsaStructInDb};
    use crate::zalsa::Zalsa;
    use crate::zalsa_local::persistence::PersistentQueryOrigin;
    use crate::zalsa_local::{QueryEdge, QueryOriginRef};
    use crate::{Id, IngredientIndex};

    use serde::de;
    use serde::ser::SerializeMap;

    use std::ptr::NonNull;

    pub struct SerializeIngredient<'db, C>
    where
        C: Configuration,
    {
        pub zalsa: &'db Zalsa,
        pub ingredient: &'db IngredientImpl<C>,
        pub keys: Option<&'db [crate::DatabaseKeyIndex]>,
        pub identities: Option<&'db crate::persistence::IdentityRows>,
    }

    impl<C> serde::Serialize for SerializeIngredient<'_, C>
    where
        C: Configuration,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let Self {
                ingredient,
                zalsa,
                keys,
                identities,
            } = self;
            if !C::PERSIST
                && let (Some(keys), Some(identities)) = (keys, identities)
            {
                let mut map = serializer.serialize_map(Some(keys.len()))?;
                for entry in *keys {
                    let query = ingredient.database_key_index(entry.key_index());
                    map.serialize_entry(
                        &(entry.ingredient_index(), entry.key_index()),
                        &identities.0[&query],
                    )?;
                }
                return map.end();
            }
            let eligible = |memo: &Memo<C>| memo.should_serialize();
            let entries = || -> Box<dyn Iterator<Item = crate::DatabaseKeyIndex> + '_> {
                match keys {
                    Some(keys) => Box::new(keys.iter().copied()),
                    None => Box::new(<C::SalsaStruct<'_> as SalsaStructInDb>::entries(zalsa)),
                }
            };

            let count = entries()
                .filter(|entry| {
                    let memo_ingredient_index = ingredient
                        .memo_ingredient_indices
                        .get(entry.ingredient_index());

                    let memo = ingredient.get_memo_from_table_for(
                        zalsa,
                        entry.key_index(),
                        memo_ingredient_index,
                    );

                    memo.is_some_and(eligible)
                })
                .count();

            let mut map = serializer.serialize_map(Some(count))?;

            let mut visited_edges = FxHashSet::default();
            let mut flattened_edges = FxIndexSet::default();

            for entry in entries() {
                let memo_ingredient_index = ingredient
                    .memo_ingredient_indices
                    .get(entry.ingredient_index());

                let memo = ingredient.get_memo_from_table_for(
                    zalsa,
                    entry.key_index(),
                    memo_ingredient_index,
                );

                if let Some(memo) = memo.filter(|memo| eligible(memo)) {
                    // A restored memo's value and dependencies are immutable. Verification
                    // may have advanced since it was loaded; replaying its older timestamp
                    // merely asks the next reader to validate those dependencies again.
                    if keys.is_some()
                        && let Some((bytes, references)) = memo.cached_record()
                    {
                        crate::persistence::trace_origin(memo.header.origin());
                        crate::persistence::trace_saved_references(references)
                            .map_err(serde::ser::Error::custom)?;
                        map.serialize_entry(
                            &(entry.ingredient_index(), entry.key_index()),
                            &(references, bytes),
                        )?;
                        continue;
                    }
                    let origin = memo.header.origin();
                    let flattened_origin = match origin {
                        QueryOriginRef::Derived(edges)
                        | QueryOriginRef::DerivedUntracked(edges) => {
                            let mut untracked =
                                matches!(origin, QueryOriginRef::DerivedUntracked(_));
                            // SAFETY: Snapshot creation excludes writes throughout the traversal.
                            unsafe {
                                if keys.is_some() {
                                    untracked |= flatten_for_cache(
                                        zalsa,
                                        edges,
                                        &mut visited_edges,
                                        &mut flattened_edges,
                                    )
                                    .map_err(serde::ser::Error::custom)?;
                                } else {
                                    collect_minimum_serialized_edges(
                                        zalsa,
                                        edges,
                                        &mut visited_edges,
                                        &mut flattened_edges,
                                    );
                                }
                            }
                            if untracked {
                                PersistentQueryOrigin::derived_untracked(flattened_edges.drain(..))
                            } else {
                                PersistentQueryOrigin::derived(flattened_edges.drain(..))
                            }
                        }
                        QueryOriginRef::Assigned(key) => {
                            let dependency = zalsa.lookup_ingredient(key.ingredient_index());
                            if !dependency.is_persistable() && keys.is_some() {
                                return Err(serde::ser::Error::custom(format!(
                                    "assigner `{}` must be persistable",
                                    dependency.debug_name()
                                )));
                            }
                            assert!(
                                dependency.is_persistable(),
                                "specified query `{}` must be persistable",
                                dependency.debug_name()
                            );
                            PersistentQueryOrigin::assigned(key)
                        }
                    };

                    if keys.is_some() {
                        crate::persistence::trace_origin(flattened_origin.as_ref());
                    }
                    let memo = memo.with_origin(flattened_origin);

                    if keys.is_some() {
                        let key = (entry.ingredient_index(), entry.key_index());
                        if C::FINGERPRINT.is_some() {
                            map.serialize_entry(&key, &BinaryMemo(&memo))?;
                        } else {
                            let (bytes, mut references) =
                                crate::persistence::with_references(|| {
                                    crate::persistence::encode(&BinaryMemo(&memo))
                                });
                            let bytes =
                                bytes::Bytes::from(bytes.map_err(serde::ser::Error::custom)?);
                            let references = crate::persistence::encode_references(&mut references)
                                .map_err(serde::ser::Error::custom)?;
                            map.serialize_entry(&key, &(references, bytes))?;
                        }
                    } else {
                        let key = format!(
                            "{}:{}",
                            entry.ingredient_index().as_u32(),
                            entry.key_index().as_bits()
                        );
                        map.serialize_entry(&key, &memo)?;
                    }

                    visited_edges.clear();
                }
            }

            map.end()
        }
    }

    /// Keeps the nearest persisted dependency along each path. An omitted query's
    /// outputs belong to it alone. Its inputs and any untracked reads must still
    /// participate in validation of the stored caller.
    ///
    /// # Safety
    /// The database must remain frozen throughout traversal.
    unsafe fn flatten_for_cache(
        zalsa: &Zalsa,
        edges: crate::zalsa_local::QueryEdges<'_>,
        visited: &mut FxHashSet<QueryEdge>,
        flattened: &mut FxIndexSet<QueryEdge>,
    ) -> std::io::Result<bool> {
        let mut untracked = false;
        let mut pending = Vec::new();
        let retains_cache_key = |key: crate::DatabaseKeyIndex| {
            zalsa
                .lookup_ingredient(key.ingredient_index())
                .is_persistable()
        };
        for edge in edges {
            if retains_cache_key(edge.key()) {
                flattened.insert(edge);
            } else {
                pending.push(edge);
                while let Some(edge) = pending.pop() {
                    if !visited.insert(edge) {
                        continue;
                    }
                    let dependency = zalsa.lookup_ingredient(edge.key().ingredient_index());
                    if dependency.as_function().is_none() {
                        if dependency.requires_persisted_input() && !dependency.is_persistable() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                format!(
                                    "input field `{}` must be persistable",
                                    dependency.debug_name()
                                ),
                            ));
                        }
                        // SAFETY: The database is frozen.
                        unsafe {
                            dependency
                                .collect_minimum_serialized_edges(zalsa, edge, flattened, visited)
                        };
                    } else if retains_cache_key(edge.key()) {
                        flattened.insert(edge);
                    } else {
                        // SAFETY: The database is frozen, including its memo tables.
                        match unsafe { zalsa.cache_memo(edge.key()) }
                            .map(|(owner, memo)| (owner, memo.cache_origin()))
                        {
                            Some((_, QueryOriginRef::Assigned(assigner))) => {
                                pending.push(QueryEdge::input(assigner))
                            }
                            Some((_, origin)) => {
                                untracked |= matches!(origin, QueryOriginRef::DerivedUntracked(_));
                                pending.extend(origin.inputs().rev().map(QueryEdge::input));
                            }
                            None => untracked = true,
                        }
                    }
                }
            }
        }
        Ok(untracked)
    }

    /// Flatten the dependency edges before serialization.
    ///
    /// # Safety
    ///
    /// The caller must prevent database writes throughout the traversal, including query
    /// execution that can reuse slots or modify memos. Concurrent read-only access is allowed.
    unsafe fn collect_minimum_serialized_edges(
        zalsa: &Zalsa,
        edges: crate::zalsa_local::QueryEdges<'_>,
        visited_edges: &mut FxHashSet<QueryEdge>,
        flattened_edges: &mut FxIndexSet<QueryEdge>,
    ) {
        for edge in edges {
            let dependency = zalsa.lookup_ingredient(edge.key().ingredient_index());

            if dependency.is_persistable() {
                // If the dependency will be serialized, we can serialize the edge directly.
                flattened_edges.insert(edge);
            } else {
                // Otherwise, serialize the minimum edges necessary to cover the dependency.
                // SAFETY: The caller prevents database writes throughout the traversal.
                unsafe {
                    dependency.collect_minimum_serialized_edges(
                        zalsa,
                        edge,
                        flattened_edges,
                        visited_edges,
                    );
                }
            }
        }
    }

    pub struct DeserializeIngredient<'db, C>
    where
        C: Configuration,
    {
        pub zalsa: &'db Zalsa,
        pub ingredient: &'db IngredientImpl<C>,
        pub archive: Option<&'db bytes::Bytes>,
        pub identities: &'db mut crate::persistence::IdentityRows,
    }

    impl<'de, C> de::DeserializeSeed<'de> for DeserializeIngredient<'_, C>
    where
        C: Configuration,
    {
        type Value = ();

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_map(self)
        }
    }

    impl<'de, C> de::Visitor<'de> for DeserializeIngredient<'_, C>
    where
        C: Configuration,
    {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            let DeserializeIngredient {
                zalsa,
                ingredient,
                archive,
                identities,
            } = self;

            loop {
                let (ingredient_index, id) = if archive.is_some() {
                    let Some(key) = access.next_key::<(IngredientIndex, Id)>()? else {
                        break;
                    };
                    key
                } else {
                    let Some(key) = access.next_key::<&str>()? else {
                        break;
                    };
                    let (index, id) = key
                        .split_once(':')
                        .ok_or_else(|| de::Error::custom("invalid database key"))?;
                    (
                        IngredientIndex::new(index.parse().map_err(de::Error::custom)?),
                        Id::from_bits(id.parse().map_err(de::Error::custom)?),
                    )
                };
                let memo: Memo<C> = if let Some(archive) = archive {
                    access.next_value_seed(BinaryMemoSeed(
                        archive,
                        zalsa,
                        std::marker::PhantomData,
                    ))?
                } else {
                    access.next_value()?
                };

                if archive.is_some()
                    && (zalsa.table().allocated_ingredient(id) != Some(ingredient_index)
                        || !zalsa.accepts_cache_key(ingredient.index, ingredient_index))
                {
                    return Err(de::Error::custom("cached memo has an incompatible owner"));
                }
                if archive.is_some() && !C::PERSIST {
                    for &(identity, output) in memo.header.revisions.tracked_struct_ids() {
                        if zalsa.table().allocated_ingredient(output)
                            != Some(identity.ingredient_index())
                        {
                            return Err(de::Error::custom("invalid tracked output in cache"));
                        }
                        identities
                            .0
                            .entry(ingredient.database_key_index(id))
                            .or_default()
                            .push((identity, output));
                    }
                }
                let memo_ingredient_index =
                    ingredient.memo_ingredient_indices.get(ingredient_index);

                // SAFETY: All owner slots already exist and cannot be reused during restore.
                // Workers install memos atomically and never execute queries on this database.
                let memo_table = unsafe { zalsa.table().dyn_memos_unchecked(id) };

                let replaced = memo_table.insert(
                    memo_ingredient_index,
                    NonNull::from(Box::leak(Box::new(memo))),
                );
                if let Some(previous) = replaced {
                    // SAFETY: The atomic swap transferred ownership of this box. No reader
                    // can access the database until the restore has completed.
                    drop(unsafe { Box::from_raw(previous.as_ptr()) });
                    return Err(de::Error::custom("duplicate query memo in snapshot"));
                }
            }

            Ok(())
        }
    }
}
