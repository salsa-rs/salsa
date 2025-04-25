#![allow(clippy::undocumented_unsafe_blocks)] // TODO(#697) document safety

use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use crate::cycle::{CycleHeadKind, CycleHeads, CycleRecoveryStrategy, EMPTY_CYCLE_HEADS};
use crate::function::{Configuration, IngredientImpl};
use crate::key::DatabaseKeyIndex;
use crate::revision::AtomicRevision;
use crate::table::memo::MemoTableWithTypesMut;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryOrigin, QueryRevisions};
use crate::{Event, EventKind, Id, Revision};

impl<C: Configuration> IngredientImpl<C> {
    /// Memos have to be stored internally using `'static` as the database lifetime.
    /// This (unsafe) function call converts from something tied to self to static.
    /// Values transmuted this way have to be transmuted back to being tied to self
    /// when they are returned to the user.
    unsafe fn to_static<'db>(
        &'db self,
        memo: NonNull<Memo<C::Output<'db>>>,
    ) -> NonNull<Memo<C::Output<'static>>> {
        memo.cast()
    }

    /// Convert from an internal memo (which uses `'static`) to one tied to self
    /// so it can be publicly released.
    unsafe fn to_self<'db>(
        &'db self,
        memo: NonNull<Memo<C::Output<'static>>>,
    ) -> NonNull<Memo<C::Output<'db>>> {
        memo.cast()
    }

    /// Convert from an internal memo (which uses `'static`) to one tied to self
    /// so it can be publicly released.
    unsafe fn to_self_ref<'db>(
        &'db self,
        memo: &'db Memo<C::Output<'static>>,
    ) -> &'db Memo<C::Output<'db>> {
        unsafe { std::mem::transmute(memo) }
    }

    /// Inserts the memo for the given key; (atomically) overwrites and returns any previously existing memo
    ///
    /// # Safety
    ///
    /// The caller needs to make sure to not drop the returned value until no more references into
    /// the database exist as there may be outstanding borrows into the `Arc` contents.
    pub(super) unsafe fn insert_memo_into_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: NonNull<Memo<C::Output<'db>>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<Memo<C::Output<'db>>>> {
        let static_memo = unsafe { self.to_static(memo) };
        let old_static_memo = unsafe {
            zalsa
                .memo_table_for(id)
                .insert(memo_ingredient_index, static_memo)
        }?;
        Some(unsafe { self.to_self(old_static_memo) })
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &'db self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C::Output<'db>>> {
        let static_memo = zalsa.memo_table_for(id).get(memo_ingredient_index)?;

        unsafe { Some(self.to_self_ref(static_memo)) }
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, FixpointInitial,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for(
        table: MemoTableWithTypesMut<'_>,
        memo_ingredient_index: MemoIngredientIndex,
    ) {
        let map = |memo: &mut Memo<C::Output<'static>>| {
            match &memo.revisions.origin {
                QueryOrigin::Assigned(_)
                | QueryOrigin::DerivedUntracked(_)
                | QueryOrigin::FixpointInitial => {
                    // Careful: Cannot evict memos whose values were
                    // assigned as output of another query
                    // or those with untracked inputs
                    // as their values cannot be reconstructed.
                }
                QueryOrigin::Derived(_) => {
                    // Set the memo value to `None`.
                    memo.value = None;
                }
            }
        };

        table.map_memo(memo_ingredient_index, map)
    }

    pub(super) fn initial_value<'db>(
        &'db self,
        db: &'db C::DbView,
        key: Id,
    ) -> Option<C::Output<'db>> {
        match C::CYCLE_STRATEGY {
            CycleRecoveryStrategy::Fixpoint | CycleRecoveryStrategy::FallbackImmediate => {
                Some(C::cycle_initial(db, C::id_to_input(db, key)))
            }
            CycleRecoveryStrategy::Panic => None,
        }
    }
}

#[derive(Debug)]
pub struct Memo<V> {
    /// The result of the query, if we decide to memoize it.
    pub(super) value: Option<V>,

    /// Last revision when this memo was verified; this begins
    /// as the current revision.
    pub(super) verified_at: AtomicRevision,

    /// Revision information
    pub(super) revisions: QueryRevisions,
}

// Memo's are stored a lot, make sure their size is doesn't randomly increase.
// #[cfg(test)]
const _: [(); std::mem::size_of::<Memo<std::num::NonZeroUsize>>()] =
    [(); std::mem::size_of::<[usize; 13]>()];

impl<V> Memo<V> {
    pub(super) fn new(value: Option<V>, revision_now: Revision, revisions: QueryRevisions) -> Self {
        Memo {
            value,
            verified_at: AtomicRevision::from(revision_now),
            revisions,
        }
    }

    /// True if this may be a provisional cycle-iteration result.
    #[inline]
    pub(super) fn may_be_provisional(&self) -> bool {
        // Relaxed is OK here, because `verified_final` is only ever mutated in one direction (from
        // `false` to `true`), and changing it to `true` on memos with cycle heads where it was
        // ever `false` is purely an optimization; if we read an out-of-date `false`, it just means
        // we might go validate it again unnecessarily.
        !self.revisions.verified_final.load(Ordering::Relaxed)
    }

    /// Invoked when `refresh_memo` is about to return a memo to the caller; if that memo is
    /// provisional, and its cycle head is claimed by another thread, we need to wait for that
    /// other thread to complete the fixpoint iteration, and then retry fetching our own memo.
    ///
    /// Return `true` if the caller should retry, `false` if the caller should go ahead and return
    /// this memo to the caller.
    #[inline(always)]
    pub(super) fn provisional_retry(
        &self,
        db: &(impl crate::Database + ?Sized),
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
    ) -> bool {
        if !self.may_be_provisional() {
            return false;
        };
        if self.revisions.cycle_heads.is_empty() {
            return false;
        }
        return provisional_retry_cold(
            db.as_dyn_database(),
            zalsa,
            database_key_index,
            &self.revisions.cycle_heads,
        );

        #[inline(never)]
        fn provisional_retry_cold(
            db: &dyn crate::Database,
            zalsa: &Zalsa,
            database_key_index: DatabaseKeyIndex,
            cycle_heads: &CycleHeads,
        ) -> bool {
            let mut retry = false;

            let db = db.as_dyn_database();
            let hit_cycle = cycle_heads
                .into_iter()
                .filter(|&head| head.database_key_index != database_key_index)
                .any(|head| {
                    let head_index = head.database_key_index;
                    let ingredient = zalsa.lookup_ingredient(head_index.ingredient_index());
                    let cycle_head_kind = ingredient.cycle_head_kind(db, head_index.key_index());
                    if matches!(
                        cycle_head_kind,
                        CycleHeadKind::NotProvisional | CycleHeadKind::FallbackImmediate
                    ) {
                        // This cycle is already finalized, so we don't need to wait on it;
                        // keep looping through cycle heads.
                        retry = true;
                        false
                    } else if ingredient.wait_for(db, head_index.key_index()) {
                        // There's a new memo available for the cycle head; fetch our own
                        // updated memo and see if it's still provisional or if the cycle
                        // has resolved.
                        retry = true;
                        false
                    } else {
                        // We hit a cycle blocking on the cycle head; this means it's in
                        // our own active query stack and we are responsible to resolve the
                        // cycle, so go ahead and return the provisional memo.
                        true
                    }
                });
            // If `retry` is `true`, all our cycle heads (barring ourself) are complete; re-fetch
            // and we should get a non-provisional memo. If we get here and `retry` is still
            // `false`, we have no cycle heads other than ourself, so we are a provisional value of
            // the cycle head (either initial value, or from a later iteration) and should be
            // returned to caller to allow fixpoint iteration to proceed. (All cases in the loop
            // above other than "cycle head is self" are either terminal or set `retry`.)
            if hit_cycle {
                false
            } else {
                retry
            }
        }
    }

    /// Cycle heads that should be propagated to dependent queries.
    #[inline(always)]
    pub(super) fn cycle_heads(&self) -> &CycleHeads {
        if self.may_be_provisional() {
            &self.revisions.cycle_heads
        } else {
            &EMPTY_CYCLE_HEADS
        }
    }

    /// Mark memo as having been verified in the `revision_now`, which should
    /// be the current revision.
    /// The caller is responsible to update the memo's `accumulated` state if heir accumulated
    /// values have changed since.
    #[inline]
    pub(super) fn mark_as_verified<Db: ?Sized + crate::Database>(
        &self,
        db: &Db,
        revision_now: Revision,
        database_key_index: DatabaseKeyIndex,
    ) {
        db.salsa_event(&|| {
            Event::new(EventKind::DidValidateMemoizedValue {
                database_key: database_key_index,
            })
        });

        self.verified_at.store(revision_now);
    }

    pub(super) fn mark_outputs_as_verified(
        &self,
        zalsa: &Zalsa,
        db: &dyn crate::Database,
        database_key_index: DatabaseKeyIndex,
    ) {
        for output in self.revisions.origin.outputs() {
            output.mark_validated_output(zalsa, db, database_key_index);
        }
    }

    pub(super) fn tracing_debug(&self) -> impl std::fmt::Debug + '_ {
        struct TracingDebug<'a, T> {
            memo: &'a Memo<T>,
        }

        impl<T> std::fmt::Debug for TracingDebug<'_, T> {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("Memo")
                    .field(
                        "value",
                        if self.memo.value.is_some() {
                            &"Some(<value>)"
                        } else {
                            &"None"
                        },
                    )
                    .field("verified_at", &self.memo.verified_at)
                    .field("revisions", &self.memo.revisions)
                    .finish()
            }
        }

        TracingDebug { memo: self }
    }
}

impl<V: Send + Sync + Any> crate::table::memo::Memo for Memo<V> {
    fn origin(&self) -> &QueryOrigin {
        &self.revisions.origin
    }
}
