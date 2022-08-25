use std::{fmt, sync::Arc};

use arc_swap::ArcSwap;
use crossbeam::{atomic::AtomicCell, queue::SegQueue};

use crate::{
    cycle::CycleRecoveryStrategy,
    ingredient::{fmt_index, IngredientRequiresReset},
    jar::Jar,
    key::{DatabaseKeyIndex, DependencyIndex},
    runtime::local_state::QueryOrigin,
    salsa_struct::SalsaStructInDb,
    Cycle, DbWithJar, Event, EventKind, Id, Revision,
};

use super::{ingredient::Ingredient, routes::IngredientIndex, AsId};

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
mod store;
mod sync;

/// Function ingredients are the "workhorse" of salsa.
/// They are used for tracked functions, for the "value" fields of tracked structs, and for the fields of input structs.
/// The function ingredient is fairly complex and so its code is spread across multiple modules, typically one per method.
/// The main entry points are:
///
/// * the `fetch` method, which is invoked when the function is called by the user's code;
///   it will return a memoized value if one exists, or execute the function otherwise.
/// * the `specify` method, which can only be used when the key is an entity created by the active query.
///   It sets the value of the function imperatively, so that when later fetches occur, they'll return this value.
/// * the `store` method, which can only be invoked with an `&mut` reference, and is to set input fields.
pub struct FunctionIngredient<C: Configuration> {
    /// The ingredient index we were assigned in the database.
    /// Used to construct `DatabaseKeyIndex` values.
    index: IngredientIndex,

    /// Tracks the keys for which we have memoized values.
    memo_map: memo::MemoMap<C::Key, C::Value>,

    /// Tracks the keys that are currently being processed; used to coordinate between
    /// worker threads.
    sync_map: sync::SyncMap,

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
    deleted_entries: SegQueue<ArcSwap<memo::Memo<C::Value>>>,

    /// Set to true once we invoke `register_dependent_fn` for `C::SalsaStruct`.
    /// Prevents us from registering more than once.
    registered: AtomicCell<bool>,

    debug_name: &'static str,
}

pub trait Configuration {
    type Jar: for<'db> Jar<'db>;

    /// The "salsa struct type" that this function is associated with.
    /// This can be just `salsa::Id` for functions that intern their arguments
    /// and are not clearly associated with any one salsa struct.
    type SalsaStruct: for<'db> SalsaStructInDb<DynDb<'db, Self>>;

    /// What key is used to index the memo. Typically a salsa struct id,
    /// but if this memoized function has multiple arguments it will be a `salsa::Id`
    /// that results from interning those arguments.
    type Key: AsId;

    /// The value computed by the function.
    type Value: fmt::Debug;

    /// Determines whether this function can recover from being a participant in a cycle
    /// (and, if so, how).
    const CYCLE_STRATEGY: CycleRecoveryStrategy;

    /// Invokes after a new result `new_value`` has been computed for which an older memoized
    /// value existed `old_value`. Returns true if the new value is equal to the older one
    /// and hence should be "backdated" (i.e., marked as having last changed in an older revision,
    /// even though it was recomputed).
    ///
    /// This invokes user's code in form of the `Eq` impl.
    fn should_backdate_value(old_value: &Self::Value, new_value: &Self::Value) -> bool;

    /// Invoked when we need to compute the value for the given key, either because we've never
    /// computed it before or because the old one relied on inputs that have changed.
    ///
    /// This invokes the function the user wrote.
    fn execute(db: &DynDb<Self>, key: Self::Key) -> Self::Value;

    /// If the cycle strategy is `Recover`, then invoked when `key` is a participant
    /// in a cycle to find out what value it should have.
    ///
    /// This invokes the recovery function given by the user.
    fn recover_from_cycle(db: &DynDb<Self>, cycle: &Cycle, key: Self::Key) -> Self::Value;

    /// Given a salsa Id, returns the key. Convenience function to avoid
    /// having to type `<C::Key as AsId>::from_id`.
    fn key_from_id(id: Id) -> Self::Key {
        AsId::from_id(id)
    }
}

/// True if `old_value == new_value`. Invoked by the generated
/// code for `should_backdate_value` so as to give a better
/// error message.
pub fn should_backdate_value<V: Eq>(old_value: &V, new_value: &V) -> bool {
    old_value == new_value
}

pub type DynDb<'bound, C> = <<C as Configuration>::Jar as Jar<'bound>>::DynDb;

/// This type is used to make configuration types for the functions in entities;
/// e.g. you can do `Config<X, 0>` and `Config<X, 1>`.
pub struct Config<const C: usize>(std::marker::PhantomData<[(); C]>);

impl<C> FunctionIngredient<C>
where
    C: Configuration,
{
    pub fn new(index: IngredientIndex, debug_name: &'static str) -> Self {
        Self {
            index,
            memo_map: memo::MemoMap::default(),
            lru: Default::default(),
            sync_map: Default::default(),
            deleted_entries: Default::default(),
            registered: Default::default(),
            debug_name,
        }
    }

    fn database_key_index(&self, k: C::Key) -> DatabaseKeyIndex {
        DatabaseKeyIndex {
            ingredient_index: self.index,
            key_index: k.as_id(),
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
    unsafe fn extend_memo_lifetime<'this, 'memo>(
        &'this self,
        memo: &'memo memo::Memo<C::Value>,
    ) -> Option<&'this C::Value> {
        let memo_value: Option<&'memo C::Value> = memo.value.as_ref();
        std::mem::transmute(memo_value)
    }

    fn insert_memo(
        &self,
        db: &DynDb<'_, C>,
        key: C::Key,
        memo: memo::Memo<C::Value>,
    ) -> Option<&C::Value> {
        self.register(db);
        let memo = Arc::new(memo);
        let value = unsafe {
            // Unsafety conditions: memo must be in the map (it's not yet, but it will be by the time this
            // value is returned) and anything removed from map is added to deleted entries (ensured elsewhere).
            self.extend_memo_lifetime(&memo)
        };
        if let Some(old_value) = self.memo_map.insert(key, memo) {
            // In case there is a reference to the old memo out there, we have to store it
            // in the deleted entries. This will get cleared when a new revision starts.
            self.deleted_entries.push(old_value);
        }
        value
    }

    /// Register this function as a dependent fn of the given salsa struct.
    /// When instances of that salsa struct are deleted, we'll get a callback
    /// so we can remove any data keyed by them.
    fn register(&self, db: &DynDb<'_, C>) {
        if !self.registered.fetch_or(true) {
            <C::SalsaStruct as SalsaStructInDb<_>>::register_dependent_fn(db, self.index)
        }
    }
}

impl<DB, C> Ingredient<DB> for FunctionIngredient<C>
where
    DB: ?Sized + DbWithJar<C::Jar>,
    C: Configuration,
{
    fn maybe_changed_after(&self, db: &DB, input: DependencyIndex, revision: Revision) -> bool {
        let key = C::key_from_id(input.key_index.unwrap());
        let db = db.as_jar_db();
        self.maybe_changed_after(db, key, revision)
    }

    fn cycle_recovery_strategy(&self) -> CycleRecoveryStrategy {
        C::CYCLE_STRATEGY
    }

    fn origin(&self, key_index: Id) -> Option<QueryOrigin> {
        let key = C::key_from_id(key_index);
        self.origin(key)
    }

    fn mark_validated_output(
        &self,
        db: &DB,
        executor: DatabaseKeyIndex,
        output_key: Option<crate::Id>,
    ) {
        let output_key = C::key_from_id(output_key.unwrap());
        self.validate_specified_value(db.as_jar_db(), executor, output_key);
    }

    fn remove_stale_output(
        &self,
        _db: &DB,
        _executor: DatabaseKeyIndex,
        _stale_output_key: Option<crate::Id>,
    ) {
        // This function is invoked when a query Q specifies the value for `stale_output_key` in rev 1,
        // but not in rev 2. We don't do anything in this case, we just leave the (now stale) memo.
        // Since its `verified_at` field has not changed, it will be considered dirty if it is invoked.
    }

    fn reset_for_new_revision(&mut self) {
        std::mem::take(&mut self.deleted_entries);
    }

    fn salsa_struct_deleted(&self, db: &DB, id: crate::Id) {
        // Remove any data keyed by `id`, since `id` no longer
        // exists in this revision.

        let id: C::Key = C::key_from_id(id);
        if let Some(origin) = self.delete_memo(id) {
            let key = self.database_key_index(id);
            db.salsa_event(Event {
                runtime_id: db.runtime().id(),
                kind: EventKind::DidDiscard { key },
            });

            // Anything that was output by this memoized execution
            // is now itself stale.
            for stale_output in origin.outputs() {
                db.remove_stale_output(key, stale_output)
            }
        }
    }

    fn fmt_index(&self, index: Option<crate::Id>, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_index(self.debug_name, index, fmt)
    }
}

impl<C> IngredientRequiresReset for FunctionIngredient<C>
where
    C: Configuration,
{
    const RESET_ON_NEW_REVISION: bool = true;
}
