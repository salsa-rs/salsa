mod lru;
mod noop;

pub use lru::{Lru, LruEntry};
pub use noop::{NoopEntry, NoopEviction};

use std::cell::UnsafeCell;
use std::hash::Hash;
use std::num::NonZeroUsize;

use crate::durability::Durability;
use crate::function::VerifyResult;
use crate::ingredient::Ingredient;
use crate::plumbing::ZalsaLocal;
use crate::table::memo::MemoTable;
use crate::zalsa::Zalsa;
use crate::{DatabaseKeyIndex, Event, EventKind, Id, Revision};

use super::{Configuration, HashEqLike, IngredientImpl, Value, ValueKey};

/// Compile-time eviction policy for an interned ingredient.
pub trait EvictionPolicy: Send + Sync + 'static {
    /// Per-shard eviction state.
    type Shard: Default + Send;

    /// Per-value eviction state.
    type Entry: Send;

    /// Per-value durability state.
    type Durability: Send;

    /// Creates the ingredient-wide eviction state.
    fn new(revisions: NonZeroUsize) -> Self;

    /// Creates the per-value eviction state.
    fn new_entry(id: Id, last_interned_at: Revision) -> Self::Entry;

    /// Creates the per-value durability state.
    fn new_durability(durability: Durability) -> Self::Durability;

    /// Returns the initial durability and last-interned revision for a new value.
    fn initial_metadata(
        zalsa_local: &ZalsaLocal,
        current_revision: Revision,
    ) -> (Durability, Revision);

    /// Returns the stable ID stored on an interned value.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock or have exclusive database access.
    unsafe fn id(entry: &Self::Entry) -> Id;

    /// Returns the metadata serialized for an interned value.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock or have exclusive database access.
    unsafe fn serialized_metadata(
        entry: &Self::Entry,
        durability: &Self::Durability,
    ) -> (Durability, Revision);

    /// Records an active revision.
    fn record_revision(&self, revision: Revision);

    /// Records an access to an existing value.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock. `entry` must have provenance derived from the
    /// enclosing value so that the policy can retain it in intrusive storage.
    unsafe fn intern_existing(&self, existing: InternExisting<'_, Self>) -> Id
    where
        Self: Sized;

    /// Interns a missing value, reusing a stale slot if this policy supports it.
    fn intern_missing<'db, C, Key, Assemble>(
        &self,
        missing: InternMissing<'_, 'db, C, Key, Assemble>,
    ) -> Id
    where
        Self: Sized,
        C: Configuration<Eviction = Self>,
        Key: Hash,
        C::Fields<'db>: HashEqLike<Key>,
        Assemble: FnOnce(Id, Key) -> C::Fields<'db>;

    /// Adds a newly allocated or restored value to the policy's per-shard state.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock. `entry` must have provenance derived from the
    /// enclosing value so that the policy can retain it in intrusive storage.
    unsafe fn insert_entry(
        &self,
        shard: &mut Self::Shard,
        entry: *const Self::Entry,
        durability: &Self::Durability,
    );

    /// Reports any dependency or query-stamp update required by a newly interned value.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock.
    unsafe fn report_tracked_read(
        &self,
        zalsa_local: &ZalsaLocal,
        index: DatabaseKeyIndex,
        current_revision: Revision,
        durability: &Self::Durability,
    );

    /// Returns whether the value is safe to expose in the current revision.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock.
    unsafe fn is_valid(
        &self,
        zalsa: &Zalsa,
        entry: &Self::Entry,
        durability: &Self::Durability,
    ) -> bool;

    /// Validates an interned dependency, returning whether its slot was reused.
    ///
    /// # Safety
    ///
    /// The caller must hold the value's shard lock.
    unsafe fn maybe_changed_after(
        &self,
        zalsa: &Zalsa,
        index: DatabaseKeyIndex,
        input: Id,
        current_revision: Revision,
        entry: &Self::Entry,
    ) -> VerifyResult;

    /// Returns whether this policy can invalidate an interned dependency.
    fn can_reuse(&self) -> bool;
}

/// Context for a lookup that found an existing interned value.
pub struct InternExisting<'a, E: EvictionPolicy> {
    pub(super) zalsa: &'a Zalsa,
    pub(super) zalsa_local: &'a ZalsaLocal,
    pub(super) index: DatabaseKeyIndex,
    pub(super) current_revision: Revision,
    pub(super) entry: *const E::Entry,
    pub(super) durability: &'a E::Durability,
    pub(super) shard: &'a mut E::Shard,
}

/// Context for a lookup that needs to allocate or reuse an interned value.
pub struct InternMissing<'a, 'db, C: Configuration, Key, Assemble> {
    pub(super) ingredient: &'db IngredientImpl<C>,
    pub(super) zalsa: &'db Zalsa,
    pub(super) zalsa_local: &'db ZalsaLocal,
    pub(super) key: Key,
    pub(super) assemble: Assemble,
    pub(super) key_map: &'a mut hashbrown::HashTable<ValueKey>,
    pub(super) shard: &'a mut <C::Eviction as EvictionPolicy>::Shard,
    pub(super) shard_index: usize,
    pub(super) hash: u64,
    pub(super) current_revision: Revision,
}

impl<'db, C, Key, Assemble> InternMissing<'_, 'db, C, Key, Assemble>
where
    C: Configuration,
    Key: Hash,
    C::Fields<'db>: HashEqLike<Key>,
    Assemble: FnOnce(Id, Key) -> C::Fields<'db>,
{
    /// Allocates a new interned value for this miss.
    pub(super) fn intern_new(self) -> Id {
        let Self {
            ingredient,
            zalsa,
            zalsa_local,
            key,
            assemble,
            key_map,
            shard,
            shard_index,
            hash,
            current_revision,
        } = self;

        let (durability, last_interned_at) =
            C::Eviction::initial_metadata(zalsa_local, current_revision);

        let (id, value) =
            zalsa_local.allocate(zalsa, ingredient.ingredient_index, |id| Value::<C> {
                shard: shard_index as u16,
                eviction: C::Eviction::new_entry(id, last_interned_at),
                // SAFETY: `from_internal_data` restores the correct lifetime before access.
                fields: UnsafeCell::new(unsafe { ingredient.to_internal_data(assemble(id, key)) }),
                // SAFETY: We only access memos allocated through this ingredient's table types.
                memos: UnsafeCell::new(unsafe { MemoTable::new(ingredient.memo_table_types()) }),
                durability: C::Eviction::new_durability(durability),
            });

        // SAFETY: We hold the shard lock and `value` is the live value just allocated in it.
        unsafe { ingredient.insert_value(key_map, shard, hash, value) };

        let index = ingredient.database_key_index(id);
        // SAFETY: We still hold the lock for the shard containing the newly allocated value.
        unsafe {
            ingredient.eviction.report_tracked_read(
                zalsa_local,
                index,
                current_revision,
                &value.durability,
            )
        };

        zalsa.event(&|| {
            Event::new(EventKind::DidInternValue {
                key: index,
                revision: current_revision,
            })
        });

        id
    }
}

/// Returns a pointer to `value.eviction` with provenance derived from `value`.
#[inline]
pub(super) fn entry_ptr<C: Configuration>(
    value: &Value<C>,
) -> *const <C::Eviction as EvictionPolicy>::Entry {
    let value = std::ptr::from_ref(value).cast::<u8>();

    // SAFETY: `eviction` is a field within `value`, so adding its offset is in bounds. Starting
    // from `value` permits an intrusive policy to recover the enclosing allocation.
    unsafe { value.add(std::mem::offset_of!(Value<C>, eviction)).cast() }
}
