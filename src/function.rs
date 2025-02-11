use std::{any::Any, fmt, mem::ManuallyDrop, sync::Arc};

use crate::{
    accumulator::accumulated_map::{AccumulatedMap, InputAccumulatedValues},
    cycle::CycleRecoveryStrategy,
    ingredient::{fmt_index, MaybeChangedAfter},
    key::DatabaseKeyIndex,
    plumbing::JarAux,
    salsa_struct::SalsaStructInDb,
    table::Table,
    zalsa::{IngredientIndex, MemoIngredientIndex, Zalsa},
    zalsa_local::QueryOrigin,
    Cycle, Database, Id, Revision,
};

use self::delete::DeletedEntries;

use super::ingredient::Ingredient;

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

pub trait Configuration: Any {
    const DEBUG_NAME: &'static str;

    /// The database that this function is associated with.
    type DbView: ?Sized + crate::Database;

    /// The "salsa struct type" that this function is associated with.
    /// This can be just `salsa::Id` for functions that intern their arguments
    /// and are not clearly associated with any one salsa struct.
    type SalsaStruct<'db>: SalsaStructInDb;

    /// The input to the function
    type Input<'db>: Send + Sync;

    /// The value computed by the function.
    type Output<'db>: fmt::Debug + Send + Sync;

    /// Determines whether this function can recover from being a participant in a cycle
    /// (and, if so, how).
    const CYCLE_STRATEGY: CycleRecoveryStrategy;

    /// Invokes after a new result `new_value`` has been computed for which an older memoized
    /// value existed `old_value`. Returns true if the new value is equal to the older one
    /// and hence should be "backdated" (i.e., marked as having last changed in an older revision,
    /// even though it was recomputed).
    ///
    /// This invokes user's code in form of the `Eq` impl.
    fn should_backdate_value(old_value: &Self::Output<'_>, new_value: &Self::Output<'_>) -> bool;

    /// Convert from the id used internally to the value that execute is expecting.
    /// This is a no-op if the input to the function is a salsa struct.
    fn id_to_input(db: &Self::DbView, key: Id) -> Self::Input<'_>;

    /// Invoked when we need to compute the value for the given key, either because we've never
    /// computed it before or because the old one relied on inputs that have changed.
    ///
    /// This invokes the function the user wrote.
    fn execute<'db>(db: &'db Self::DbView, input: Self::Input<'db>) -> Self::Output<'db>;

    /// If the cycle strategy is `Fallback`, then invoked when `key` is a participant
    /// in a cycle to find out what value it should have.
    ///
    /// This invokes the recovery function given by the user.
    fn recover_from_cycle<'db>(
        db: &'db Self::DbView,
        cycle: &Cycle,
        input: Self::Input<'db>,
    ) -> Self::Output<'db>;
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
    memo_ingredient_index: MemoIngredientIndex,

    /// Used to find memos to throw out when we have too many memoized values.
    lru: lru::Lru,

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

/// True if `old_value == new_value`. Invoked by the generated
/// code for `should_backdate_value` so as to give a better
/// error message.
pub fn should_backdate_value<V: Eq>(old_value: &V, new_value: &V) -> bool {
    old_value == new_value
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub fn new(struct_index: IngredientIndex, index: IngredientIndex, aux: &dyn JarAux) -> Self {
        Self {
            index,
            memo_ingredient_index: aux.next_memo_ingredient_index(struct_index, index),
            lru: Default::default(),
            deleted_entries: Default::default(),
        }
    }

    pub fn database_key_index(&self, k: Id) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.index,
            key_index: k,
        }
    }

    pub fn set_capacity(&self, capacity: usize) {
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
        std::mem::transmute(memo)
    }

    fn insert_memo<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: memo::Memo<C::Output<'db>>,
    ) -> &'db memo::Memo<C::Output<'db>> {
        let memo = Arc::new(memo);
        // Unsafety conditions: memo must be in the map (it's not yet, but it will be by the time this
        // value is returned) and anything removed from map is added to deleted entries (ensured elsewhere).
        let db_memo = unsafe { self.extend_memo_lifetime(&memo) };
        // Safety: We delay the drop of `old_value` until a new revision starts which ensures no
        // references will exist for the memo contents.
        if let Some(old_value) = unsafe { self.insert_memo_into_table_for(zalsa, id, memo) } {
            // In case there is a reference to the old memo out there, we have to store it
            // in the deleted entries. This will get cleared when a new revision starts.
            self.deleted_entries
                .push(ManuallyDrop::into_inner(old_value));
        }
        db_memo
    }
}

impl<C> Ingredient for IngredientImpl<C>
where
    C: Configuration,
{
    fn ingredient_index(&self) -> IngredientIndex {
        self.index
    }

    fn maybe_changed_after(
        &self,
        db: &dyn Database,
        input: Id,
        revision: Revision,
    ) -> MaybeChangedAfter {
        let db = db.as_view::<C::DbView>();
        self.maybe_changed_after(db, input, revision)
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        C::CYCLE_STRATEGY
    }

    fn origin(&self, db: &dyn Database, key: Id) -> Option<QueryOrigin> {
        self.origin(db.zalsa(), key)
    }

    fn mark_validated_output(
        &self,
        db: &dyn Database,
        executor: DatabaseKeyIndex,
        output_key: crate::Id,
    ) {
        self.validate_specified_value(db, executor, output_key);
    }

    fn remove_stale_output(
        &self,
        _db: &dyn Database,
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
        self.lru
            .for_each_evicted(|evict| self.evict_value_from_memo_for(table.memos_mut(evict)));
        std::mem::take(&mut self.deleted_entries);
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(C::DEBUG_NAME, index, fmt)
    }

    fn debug_name(&self) -> &'static str {
        C::DEBUG_NAME
    }

    fn accumulated<'db>(
        &'db self,
        db: &'db dyn Database,
        key_index: Id,
    ) -> (Option<&'db AccumulatedMap>, InputAccumulatedValues) {
        let db = db.as_view::<C::DbView>();
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
