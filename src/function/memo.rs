use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::mem::transmute;
use std::ptr::NonNull;

use crate::cycle::{empty_cycle_heads, CycleHead, CycleHeadKind, CycleHeads};
use crate::function::{Configuration, IngredientImpl};
use crate::hash::FxHashSet;
use crate::ingredient::{Ingredient, WaitForResult};
use crate::key::DatabaseKeyIndex;
use crate::revision::AtomicRevision;
use crate::runtime::BlockedOn;
use crate::sync::atomic::Ordering;
use crate::table::memo::MemoTableWithTypesMut;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{QueryOriginRef, QueryRevisions, ZalsaLocal};
use crate::{Event, EventKind, Id, Revision};

impl<C: Configuration> IngredientImpl<C> {
    /// Inserts the memo for the given key; (atomically) overwrites and returns any previously existing memo
    pub(super) fn insert_memo_into_table_for<'db>(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo: NonNull<Memo<C::Output<'db>>>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<NonNull<Memo<C::Output<'db>>>> {
        // SAFETY: The table stores 'static memos (to support `Any`), the memos are in fact valid
        // for `'db` though as we delay their dropping to the end of a revision.
        let static_memo = unsafe {
            transmute::<NonNull<Memo<C::Output<'db>>>, NonNull<Memo<C::Output<'static>>>>(memo)
        };
        let old_static_memo = zalsa
            .memo_table_for(id)
            .insert(memo_ingredient_index, static_memo)?;
        // SAFETY: The table stores 'static memos (to support `Any`), the memos are in fact valid
        // for `'db` though as we delay their dropping to the end of a revision.
        Some(unsafe {
            transmute::<NonNull<Memo<C::Output<'static>>>, NonNull<Memo<C::Output<'db>>>>(
                old_static_memo,
            )
        })
    }

    /// Loads the current memo for `key_index`. This does not hold any sort of
    /// lock on the `memo_map` once it returns, so this memo could immediately
    /// become outdated if other threads store into the `memo_map`.
    pub(super) fn get_memo_from_table_for<'db>(
        &self,
        zalsa: &'db Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Option<&'db Memo<C::Output<'db>>> {
        let static_memo = zalsa.memo_table_for(id).get(memo_ingredient_index)?;
        // SAFETY: The table stores 'static memos (to support `Any`), the memos are in fact valid
        // for `'db` though as we delay their dropping to the end of a revision.
        Some(unsafe {
            transmute::<&Memo<C::Output<'static>>, &'db Memo<C::Output<'db>>>(static_memo.as_ref())
        })
    }

    /// Evicts the existing memo for the given key, replacing it
    /// with an equivalent memo that has no value. If the memo is untracked, FixpointInitial,
    /// or has values assigned as output of another query, this has no effect.
    pub(super) fn evict_value_from_memo_for(
        table: MemoTableWithTypesMut<'_>,
        memo_ingredient_index: MemoIngredientIndex,
    ) {
        let map = |memo: &mut Memo<C::Output<'static>>| {
            match memo.revisions.origin.as_ref() {
                QueryOriginRef::Assigned(_)
                | QueryOriginRef::DerivedUntracked(_)
                | QueryOriginRef::FixpointInitial => {
                    // Careful: Cannot evict memos whose values were
                    // assigned as output of another query
                    // or those with untracked inputs
                    // as their values cannot be reconstructed.
                }
                QueryOriginRef::Derived(_) => {
                    // Set the memo value to `None`.
                    memo.value = None;
                }
            }
        };

        table.map_memo(memo_ingredient_index, map)
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
#[cfg(not(feature = "shuttle"))]
#[cfg(target_pointer_width = "64")]
const _: [(); std::mem::size_of::<Memo<std::num::NonZeroUsize>>()] =
    [(); std::mem::size_of::<[usize; 6]>()];

impl<V> Memo<V> {
    pub(super) fn new(value: Option<V>, revision_now: Revision, revisions: QueryRevisions) -> Self {
        debug_assert!(
            !revisions.verified_final.load(Ordering::Relaxed) || revisions.cycle_heads().is_empty(),
            "Memo must be finalized if it has no cycle heads"
        );
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
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
    ) -> bool {
        if self.revisions.cycle_heads().is_empty() {
            return false;
        }

        if !self.may_be_provisional() {
            return false;
        };

        if self.block_on_heads(zalsa, zalsa_local) {
            // If we get here, we are a provisional value of
            // the cycle head (either initial value, or from a later iteration) and should be
            // returned to caller to allow fixpoint iteration to proceed.
            false
        } else {
            // all our cycle heads are complete; re-fetch
            // and we should get a non-provisional memo.
            tracing::debug!(
                "Retrying provisional memo {database_key_index:?} after awaiting cycle heads."
            );
            true
        }
    }

    /// Blocks on all cycle heads (recursively) that this memo depends on.
    ///
    /// Returns `true` if awaiting all cycle heads results in a cycle. This means, they're all waiting
    /// for us to make progress.
    #[inline(always)]
    pub(super) fn block_on_heads(&self, zalsa: &Zalsa, zalsa_local: &ZalsaLocal) -> bool {
        // IMPORTANT: If you make changes to this function, make sure to run `cycle_nested_deep` with
        // shuttle with at least 10k iterations.

        // The most common case is that the entire cycle is running in the same thread.
        // If that's the case, short circuit and return `true` immediately.
        if self.all_cycles_on_stack(zalsa_local) {
            return true;
        }

        // Otherwise, await all cycle heads, recursively.
        return block_on_heads_cold(zalsa, self.cycle_heads());

        #[inline(never)]
        fn block_on_heads_cold(zalsa: &Zalsa, heads: &CycleHeads) -> bool {
            let mut cycle_heads = TryClaimCycleHeadsIter::new(zalsa, heads);
            let mut all_cycles = true;

            while let Some(claim_result) = cycle_heads.next() {
                match claim_result {
                    TryClaimHeadsResult::Cycle => {}
                    TryClaimHeadsResult::Finalized => {
                        all_cycles = false;
                    }
                    TryClaimHeadsResult::Available => {
                        all_cycles = false;
                    }
                    TryClaimHeadsResult::Running(running) => {
                        all_cycles = false;
                        running.block_on(&mut cycle_heads);
                    }
                }
            }

            all_cycles
        }
    }

    /// Tries to claim all cycle heads to see if they're finalized or available.
    ///
    /// Unlike `block_on_heads`, this code does not block on any cycle head. Instead it returns `false` if
    /// claiming all cycle heads failed because one of them is running on another thread.
    pub(super) fn try_claim_heads(&self, zalsa: &Zalsa, zalsa_local: &ZalsaLocal) -> bool {
        if self.all_cycles_on_stack(zalsa_local) {
            return true;
        }

        let cycle_heads = TryClaimCycleHeadsIter::new(zalsa, self.revisions.cycle_heads());

        for claim_result in cycle_heads {
            match claim_result {
                TryClaimHeadsResult::Cycle
                | TryClaimHeadsResult::Finalized
                | TryClaimHeadsResult::Available => {}
                TryClaimHeadsResult::Running(_) => {
                    return false;
                }
            }
        }

        true
    }

    fn all_cycles_on_stack(&self, zalsa_local: &ZalsaLocal) -> bool {
        let cycle_heads = self.revisions.cycle_heads();
        if cycle_heads.is_empty() {
            return true;
        }

        zalsa_local.with_query_stack(|stack| {
            cycle_heads.iter().all(|cycle_head| {
                stack
                    .iter()
                    .rev()
                    .any(|query| query.database_key_index == cycle_head.database_key_index)
            })
        })
    }

    /// Cycle heads that should be propagated to dependent queries.
    #[inline(always)]
    pub(super) fn cycle_heads(&self) -> &CycleHeads {
        if self.may_be_provisional() {
            self.revisions.cycle_heads()
        } else {
            empty_cycle_heads()
        }
    }

    /// Mark memo as having been verified in the `revision_now`, which should
    /// be the current revision.
    /// The caller is responsible to update the memo's `accumulated` state if their accumulated
    /// values have changed since.
    #[inline]
    pub(super) fn mark_as_verified(&self, zalsa: &Zalsa, database_key_index: DatabaseKeyIndex) {
        zalsa.event(&|| {
            Event::new(EventKind::DidValidateMemoizedValue {
                database_key: database_key_index,
            })
        });

        self.verified_at.store(zalsa.current_revision());
    }

    pub(super) fn mark_outputs_as_verified(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
    ) {
        for output in self.revisions.origin.as_ref().outputs() {
            output.mark_validated_output(zalsa, database_key_index);
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
    fn origin(&self) -> QueryOriginRef<'_> {
        self.revisions.origin.as_ref()
    }
}

pub(super) enum TryClaimHeadsResult<'me> {
    /// Claiming every cycle head results in a cycle head.
    Cycle,

    /// The cycle head has been finalized.
    Finalized,

    /// The cycle head is not finalized, but it can be claimed.
    Available,

    /// The cycle head is currently executed on another thread.
    Running(RunningCycleHead<'me>),
}

pub(super) struct RunningCycleHead<'me> {
    blocked_on: BlockedOn<'me>,
    ingredient: &'me dyn Ingredient,
}

impl<'a> RunningCycleHead<'a> {
    fn block_on(self, cycle_heads: &mut TryClaimCycleHeadsIter<'a>) {
        let key_index = self.blocked_on.database_key().key_index();
        self.blocked_on.wait_for(cycle_heads.zalsa);

        cycle_heads.queue_ingredient_heads(self.ingredient, key_index);
    }
}

/// Iterator to try claiming the transitive cycle heads of a memo.
struct TryClaimCycleHeadsIter<'a> {
    zalsa: &'a Zalsa,
    queue: Vec<CycleHead>,
    queued: FxHashSet<CycleHead>,
}

impl<'a> TryClaimCycleHeadsIter<'a> {
    fn new(zalsa: &'a Zalsa, heads: &CycleHeads) -> Self {
        let queue: Vec<_> = heads.iter().copied().collect();
        let queued: FxHashSet<_> = queue.iter().copied().collect();

        Self {
            zalsa,
            queue,
            queued,
        }
    }

    fn queue_ingredient_heads(&mut self, ingredient: &dyn Ingredient, key: Id) {
        // Recursively wait for all cycle heads that this head depends on.
        // This is normally not necessary, because cycle heads are transitively added
        // as query dependencies (they aggregate). The exception to this are queries
        // that depend on a fixpoint initial value. They only depend on the fixpoint initial
        // value but not on its dependencies because they aren't known yet. They're only known
        // once the cycle completes but the cycle heads of the queries don't get updated.
        // Because of that, recurse here to collect all cycle heads.
        // This also ensures that if a query added new cycle heads, that they are awaited too.
        // IMPORTANT: It's critical that we get the cycle head from the latest memo
        // here, in case the memo has become part of another cycle (we need to block on that too!).
        self.queue.extend(
            ingredient
                .cycle_heads(self.zalsa, key)
                .iter()
                .copied()
                .filter(|head| self.queued.insert(*head)),
        )
    }
}

impl<'me> Iterator for TryClaimCycleHeadsIter<'me> {
    type Item = TryClaimHeadsResult<'me>;

    fn next(&mut self) -> Option<Self::Item> {
        let head = self.queue.pop()?;

        let head_database_key = head.database_key_index;
        let head_key_index = head_database_key.key_index();
        let ingredient = self
            .zalsa
            .lookup_ingredient(head_database_key.ingredient_index());
        // We don't care about the iteration. If it's final, we can go.
        let cycle_head_kind = ingredient.cycle_head_kind(self.zalsa, head_key_index, None);

        match cycle_head_kind {
            CycleHeadKind::Final | CycleHeadKind::FallbackImmediate => {
                // This cycle is already finalized, so we don't need to wait on it;
                // keep looping through cycle heads.
                tracing::trace!("Dependent cycle head {head:?} has been finalized.");
                Some(TryClaimHeadsResult::Finalized)
            }
            CycleHeadKind::Provisional => {
                match ingredient.wait_for(self.zalsa, head_key_index) {
                    WaitForResult::Cycle => {
                        // We hit a cycle blocking on the cycle head; this means this query actively
                        // participates in the cycle and some other query is blocked on this thread.
                        tracing::debug!("Waiting for {head:?} results in a cycle");
                        Some(TryClaimHeadsResult::Cycle)
                    }
                    WaitForResult::Running(running) => {
                        tracing::debug!(
                            "Ingredient {head:?} is running: {running:?}, blocking on it"
                        );

                        Some(TryClaimHeadsResult::Running(RunningCycleHead {
                            blocked_on: running.into_blocked_on(),
                            ingredient,
                        }))
                    }
                    WaitForResult::Available => {
                        self.queue_ingredient_heads(ingredient, head_key_index);
                        Some(TryClaimHeadsResult::Available)
                    }
                }
            }
        }
    }
}
