pub(crate) use maybe_changed_after::VerifyResult;
use std::any::Any;
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
pub(crate) use sync::SyncGuard;

use crate::accumulator::accumulated_map::{AccumulatedMap, InputAccumulatedValues};
use crate::cycle::{
    empty_cycle_heads, CycleHeads, CycleRecoveryAction, CycleRecoveryStrategy, ProvisionalStatus,
};
use crate::function::delete::DeletedEntries;
use crate::function::sync::{ClaimResult, SyncTable};
use crate::ingredient::{Ingredient, WaitForResult};
use crate::key::DatabaseKeyIndex;
use crate::plumbing::MemoIngredientMap;
use crate::salsa_struct::SalsaStructInDb;
use crate::sync::Arc;
use crate::table::memo::MemoTableTypes;
use crate::table::Table;
use crate::views::DatabaseDownCaster;
use crate::zalsa::{IngredientIndex, MemoIngredientIndex, Zalsa};
use crate::zalsa_local::QueryOriginRef;
use crate::{Database, Id, Revision};

mod accumulated;
mod backdate;
mod delete;
mod diff_outputs;
mod execute;
mod fetch;
mod inputs;
mod lru;
mod maybe_changed_after;
mod memo;
mod specify;
mod sync;

pub type Memo<C> = memo::Memo<<C as Configuration>::Output<'static>>;

pub trait Configuration: Any {
    const DEBUG_NAME: &'static str;
    const LOCATION: crate::ingredient::Location;

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

    /// Determines whether this function can recover from being a participant in a cycle
    /// (and, if so, how).
    const CYCLE_STRATEGY: CycleRecoveryStrategy;

    /// Invokes after a new result `new_value` has been computed for which an older memoized value
    /// existed `old_value`, or in fixpoint iteration. Returns true if the new value is equal to
    /// the older one.
    ///
    /// This invokes user code in form of the `Eq` impl.
    fn values_equal<'db>(old_value: &Self::Output<'db>, new_value: &Self::Output<'db>) -> bool;

    // FIXME: This should take a `&Zalsa`
    /// Convert from the id used internally to the value that execute is expecting.
    /// This is a no-op if the input to the function is a salsa struct.
    fn id_to_input(db: &Self::DbView, key: Id) -> Self::Input<'_>;

    /// Invoked when we need to compute the value for the given key, either because we've never
    /// computed it before or because the old one relied on inputs that have changed.
    ///
    /// This invokes the function the user wrote.
    fn execute<'db>(db: &'db Self::DbView, input: Self::Input<'db>) -> Self::Output<'db>;

    /// Get the cycle recovery initial value.
    fn cycle_initial<'db>(db: &'db Self::DbView, input: Self::Input<'db>) -> Self::Output<'db>;

    /// Decide whether to iterate a cycle again or fallback. `value` is the provisional return
    /// value from the latest iteration of this cycle. `count` is the number of cycle iterations
    /// we've already completed.
    fn recover_from_cycle<'db>(
        db: &'db Self::DbView,
        value: &Self::Output<'db>,
        count: u32,
        input: Self::Input<'db>,
    ) -> CycleRecoveryAction<Self::Output<'db>>;
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

    /// Used to find memos to throw out when we have too many memoized values.
    lru: lru::Lru,

    /// A downcaster from `dyn Database` to `C::DbView`.
    ///
    /// # Safety
    ///
    /// The supplied database must be be the same as the database used to construct the [`Views`]
    /// instances that this downcaster was derived from.
    view_caster: DatabaseDownCaster<C::DbView>,

    sync_table: SyncTable,

    /// When `fetch` and friends executes, they return a reference to the
    /// value stored in the memo that is extended to live as long as the `&self`
    /// reference we start with. This means that whenever we remove something
    /// from `memo_map` with an `&self` reference, there *could* be references to its
    /// internals still in use. Therefore we push the memo into this queue and
    /// only *actually* free up memory when a new revision starts (which means
    /// we have an `&mut` reference to self).
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
        lru: usize,
        view_caster: DatabaseDownCaster<C::DbView>,
    ) -> Self {
        Self {
            index,
            memo_ingredient_indices,
            lru: lru::Lru::new(lru),
            deleted_entries: Default::default(),
            view_caster,
            sync_table: SyncTable::new(index),
        }
    }

    #[inline]
    pub fn database_key_index(&self, key: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex::new(self.index, key)
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.lru.set_capacity(capacity);
    }

    /// Returns a reference to the memo value that lives as long as self.
    /// This is UNSAFE: the caller is responsible for ensuring that the
    /// memo will not be released so long as the `&self` is valid.
    /// This is done by (a) ensuring the memo is present in the memo-map
    /// when this function is called and (b) ensuring that any entries
    /// removed from the memo-map are added to `deleted_entries`, which is
    /// only cleared with `&mut self`.
    unsafe fn extend_memo_lifetime<'this>(
        &'this self,
        memo: &memo::Memo<C::Output<'this>>,
    ) -> &'this memo::Memo<C::Output<'this>> {
        // SAFETY: the caller must guarantee that the memo will not be released before `&self`
        unsafe { std::mem::transmute(memo) }
    }

    fn insert_memo<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: memo::Memo<C::Output<'db>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> &'db memo::Memo<C::Output<'db>> {
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
        // SAFETY: memo has been inserted into the table
        unsafe { self.extend_memo_lifetime(memo.as_ref()) }
    }

    #[inline]
    fn memo_ingredient_index(&self, zalsa: &Zalsa, id: Id) -> MemoIngredientIndex {
        self.memo_ingredient_indices.get_zalsa_id(zalsa, id)
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
        db: &dyn Database,
        input: Id,
        revision: Revision,
        cycle_heads: &mut CycleHeads,
    ) -> VerifyResult {
        // SAFETY: The `db` belongs to the ingredient as per caller invariant
        let db = unsafe { self.view_caster.downcast_unchecked(db) };
        self.maybe_changed_after(db, input, revision, cycle_heads)
    }

    /// Returns `final` only if the memo has the `verified_final` flag set and the cycle recovery strategy is not `FallbackImmediate`.
    ///
    /// Otherwise, the value is still provisional. For both final and provisional, it also
    /// returns the iteration in which this memo was created (always 0 except for cycle heads).
    fn provisional_status(&self, zalsa: &Zalsa, input: Id) -> Option<ProvisionalStatus> {
        let memo =
            self.get_memo_from_table_for(zalsa, input, self.memo_ingredient_index(zalsa, input))?;

        let iteration = memo.revisions.iteration();
        let verified_final = memo.revisions.verified_final.load(Ordering::Relaxed);

        Some(if verified_final {
            if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                ProvisionalStatus::FallbackImmediate
            } else {
                ProvisionalStatus::Final { iteration }
            }
        } else {
            ProvisionalStatus::Provisional { iteration }
        })
    }

    fn cycle_heads<'db>(&self, zalsa: &'db Zalsa, input: Id) -> &'db CycleHeads {
        self.get_memo_from_table_for(zalsa, input, self.memo_ingredient_index(zalsa, input))
            .map(|memo| memo.cycle_heads())
            .unwrap_or(empty_cycle_heads())
    }

    /// Attempts to claim `key_index` without blocking.
    ///
    /// * [`WaitForResult::Running`] if the `key_index` is running on another thread. It's up to the caller to block on the other thread
    ///   to wait until the result becomes available.
    /// * [`WaitForResult::Available`] It is (or at least was) possible to claim the `key_index`
    /// * [`WaitResult::Cycle`] Claiming the `key_index` results in a cycle because it's on the current's thread query stack or
    ///   running on another thread that is blocked on this thread.
    fn wait_for<'me>(&'me self, zalsa: &'me Zalsa, key_index: Id) -> WaitForResult<'me> {
        match self.sync_table.try_claim(zalsa, key_index) {
            ClaimResult::Running(blocked_on) => WaitForResult::Running(blocked_on),
            ClaimResult::Cycle { same_thread } => WaitForResult::Cycle { same_thread },
            ClaimResult::Claimed(_) => WaitForResult::Available,
        }
    }

    fn origin<'db>(&self, zalsa: &'db Zalsa, key: Id) -> Option<QueryOriginRef<'db>> {
        self.origin(zalsa, key)
    }

    fn mark_validated_output(
        &self,
        zalsa: &Zalsa,
        executor: DatabaseKeyIndex,
        output_key: crate::Id,
    ) {
        self.validate_specified_value(zalsa, executor, output_key);
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
        self.lru.for_each_evicted(|evict| {
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

    fn memo_table_types(&self) -> Arc<MemoTableTypes> {
        unreachable!("function does not allocate pages")
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        C::CYCLE_STRATEGY
    }

    fn accumulated<'db>(
        &'db self,
        db: &'db dyn Database,
        key_index: Id,
    ) -> (Option<&'db AccumulatedMap>, InputAccumulatedValues) {
        let db = self.view_caster.downcast(db);
        self.accumulated_map(db, key_index)
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
