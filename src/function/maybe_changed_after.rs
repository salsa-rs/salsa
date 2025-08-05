#[cfg(feature = "accumulator")]
use crate::accumulator::accumulated_map::InputAccumulatedValues;
use crate::cycle::{CycleHeadKeys, CycleRecoveryStrategy, IterationCount, ProvisionalStatus};
use crate::function::memo::Memo;
use crate::function::sync::ClaimResult;
use crate::function::{Configuration, IngredientImpl};
use crate::key::DatabaseKeyIndex;
use crate::sync::atomic::Ordering;
use crate::zalsa::{MemoIngredientIndex, Zalsa, ZalsaDatabase};
use crate::zalsa_local::{QueryEdgeKind, QueryOriginRef, ZalsaLocal};
use crate::{Id, Revision};

/// Result of memo validation.
#[derive(Debug)]
pub enum VerifyResult {
    Changed,
    Unchanged {
        #[cfg(feature = "accumulator")]
        accumulated: InputAccumulatedValues,
    },
}

impl VerifyResult {
    pub(crate) const fn changed_if(changed: bool) -> Self {
        if changed {
            Self::changed()
        } else {
            Self::unchanged()
        }
    }

    pub(crate) const fn changed() -> Self {
        Self::Changed
    }

    pub(crate) const fn unchanged() -> Self {
        Self::Unchanged {
            #[cfg(feature = "accumulator")]
            accumulated: InputAccumulatedValues::Empty,
        }
    }

    #[inline]
    #[cfg(feature = "accumulator")]
    pub(crate) fn unchanged_with_accumulated(accumulated: InputAccumulatedValues) -> Self {
        Self::Unchanged { accumulated }
    }

    #[inline]
    #[cfg(not(feature = "accumulator"))]
    pub(crate) fn unchanged_with_accumulated() -> Self {
        Self::unchanged()
    }

    pub(crate) const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged { .. })
    }
}

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    pub(super) fn maybe_changed_after<'db>(
        &'db self,
        db: &'db C::DbView,
        id: Id,
        revision: Revision,
        cycle_heads: &mut MaybeChangeAfterCycleHeads,
    ) -> VerifyResult {
        let (zalsa, zalsa_local) = db.zalsas();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);
        zalsa.unwind_if_revision_cancelled(zalsa_local);

        loop {
            let database_key_index = self.database_key_index(id);

            crate::tracing::debug!(
                "{database_key_index:?}: maybe_changed_after(revision = {revision:?})"
            );

            // Check if we have a verified version: this is the hot path.
            let memo_guard = self.get_memo_from_table_for(zalsa, id, memo_ingredient_index);
            let Some(memo) = memo_guard else {
                // No memo? Assume has changed.
                return VerifyResult::changed();
            };

            let can_shallow_update = self.shallow_verify_memo(zalsa, database_key_index, memo);
            if can_shallow_update.yes() && !memo.may_be_provisional() {
                self.update_shallow(zalsa, database_key_index, memo, can_shallow_update);

                return if memo.revisions.changed_at > revision {
                    VerifyResult::changed()
                } else {
                    VerifyResult::unchanged_with_accumulated(
                        #[cfg(feature = "accumulator")]
                        {
                            memo.revisions.accumulated_inputs.load()
                        },
                    )
                };
            }

            if let Some(mcs) = self.maybe_changed_after_cold(
                zalsa,
                db,
                id,
                revision,
                memo_ingredient_index,
                cycle_heads,
            ) {
                return mcs;
            } else {
                // We failed to claim, have to retry.
            }
        }
    }

    #[inline(never)]
    fn maybe_changed_after_cold<'db>(
        &'db self,
        zalsa: &Zalsa,
        db: &'db C::DbView,
        key_index: Id,
        revision: Revision,
        memo_ingredient_index: MemoIngredientIndex,
        cycle_heads: &mut MaybeChangeAfterCycleHeads,
    ) -> Option<VerifyResult> {
        let database_key_index = self.database_key_index(key_index);

        let _claim_guard = match self.sync_table.try_claim(zalsa, key_index) {
            ClaimResult::Claimed(guard) => guard,
            ClaimResult::Running(blocked_on) => {
                blocked_on.block_on(zalsa);
                return None;
            }
            ClaimResult::Cycle { .. } => {
                return Some(self.maybe_changed_after_cold_cycle(
                    db,
                    database_key_index,
                    cycle_heads,
                ))
            }
        };
        // Load the current memo, if any.
        let Some(old_memo) = self.get_memo_from_table_for(zalsa, key_index, memo_ingredient_index)
        else {
            return Some(VerifyResult::changed());
        };

        crate::tracing::debug!(
            "{database_key_index:?}: maybe_changed_after_cold, successful claim, \
                revision = {revision:?}, old_memo = {old_memo:#?}",
            old_memo = old_memo.tracing_debug()
        );

        let can_shallow_update = self.shallow_verify_memo(zalsa, database_key_index, old_memo);
        if can_shallow_update.yes()
            && self.validate_may_be_provisional(
                zalsa,
                db.zalsa_local(),
                database_key_index,
                old_memo,
                false,
            )
        {
            self.update_shallow(zalsa, database_key_index, old_memo, can_shallow_update);

            return Some(VerifyResult::unchanged());
        }

        // Check if the inputs are still valid. We can just compare `changed_at`.
        let deep_verify =
            self.deep_verify_memo(db, zalsa, old_memo, database_key_index, cycle_heads);

        if deep_verify.is_unchanged() {
            return Some(if old_memo.revisions.changed_at > revision {
                VerifyResult::changed()
            } else {
                deep_verify
            });
        }

        // If inputs have changed, but we have an old value, we can re-execute.
        // It is possible the result will be equal to the old value and hence
        // backdated. In that case, although we will have computed a new memo,
        // the value has not logically changed.
        //
        // However, executing the query here is only safe if there's no "unverified"
        // cycle. Let's assume we have:
        // * query `a`
        //   * depends on `b` (input)
        //   * depends on `c` (input)
        //   * creates interned `x` (input)
        // * query `c`
        //   * depends on `a` (input)
        //   * depeonds on `d` (input/changed)
        //   * reads `x`
        //
        // When `maybe_changed_after` runs on `a`, it recurses into `b` and then `c`. Now, `c` will recurse into `a` again
        // where we return `Unchanged` but add `a` to the cycle heads. `maybe_changed_after` now continues checking `d`
        // which returns `changed` so we'll reach the point here. Now, the reason we can't `execute` c
        // is because `a` hasn't re-interned `x` at this point (we started recursing into `c`).
        //
        // We can't run into this situation with normal queries because a dependency can't depent on values from
        // the outer query (it's a tree and not a graph). However, it's possible with cycles because
        // an interned or tracked struct like `x` can flow into a dependent query as part of a later iteration and
        // those values come later in `input_outputs` than the first call to the query that ultimately causes the cycle.
        if old_memo.value.is_some() && !cycle_heads.has_any() {
            let active_query = db
                .zalsa_local()
                .push_query(database_key_index, IterationCount::initial());
            let memo = self.execute(db, active_query, Some(old_memo));
            let changed_at = memo.revisions.changed_at;

            // Always assume that a provisional value has changed (we simply don't know yet
            // and we need to iterate some outer cycle to determine it, which we can't do here).
            return Some(if changed_at > revision || memo.may_be_provisional() {
                VerifyResult::changed()
            } else {
                VerifyResult::unchanged_with_accumulated(
                    #[cfg(feature = "accumulator")]
                    {
                        match memo.revisions.accumulated() {
                            Some(_) => InputAccumulatedValues::Any,
                            None => memo.revisions.accumulated_inputs.load(),
                        }
                    },
                )
            });
        }

        // Otherwise, nothing for it: have to consider the value to have changed.
        Some(deep_verify)
    }

    #[cold]
    #[inline(never)]
    fn maybe_changed_after_cold_cycle<'db>(
        &'db self,
        db: &'db C::DbView,
        database_key_index: DatabaseKeyIndex,
        cycle_heads: &mut MaybeChangeAfterCycleHeads,
    ) -> VerifyResult {
        match C::CYCLE_STRATEGY {
            // SAFETY: We do not access the query stack reentrantly.
            CycleRecoveryStrategy::Panic => unsafe {
                db.zalsa_local().with_query_stack_unchecked(|stack| {
                    panic!(
                        "dependency graph cycle when validating {database_key_index:#?}, \
                    set cycle_fn/cycle_initial to fixpoint iterate.\n\
                    Query stack:\n{stack:#?}",
                    );
                })
            },
            CycleRecoveryStrategy::FallbackImmediate => VerifyResult::unchanged(),
            CycleRecoveryStrategy::Fixpoint => {
                crate::tracing::debug!(
                    "hit cycle at {database_key_index:?} in `maybe_changed_after`,  returning fixpoint initial value",
                );
                cycle_heads.insert(database_key_index);
                VerifyResult::unchanged()
            }
        }
    }

    /// `Some` if the memo's value and `changed_at` time is still valid in this revision.
    /// Does only a shallow O(1) check, doesn't walk the dependencies.
    ///
    /// In general, a provisional memo (from cycle iteration) does not verify. Since we don't
    /// eagerly finalize all provisional memos in cycle iteration, we have to lazily check here
    /// (via `validate_provisional`) whether a may-be-provisional memo should actually be verified
    /// final, because its cycle heads are all now final.
    #[inline]
    pub(super) fn shallow_verify_memo(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<'_, C>,
    ) -> ShallowUpdate {
        crate::tracing::debug!(
            "{database_key_index:?}: shallow_verify_memo(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );
        let verified_at = memo.verified_at.load();
        let revision_now = zalsa.current_revision();

        if verified_at == revision_now {
            // Already verified.
            return ShallowUpdate::Verified;
        }

        let last_changed = zalsa.last_changed_revision(memo.revisions.durability);
        crate::tracing::debug!(
            "{database_key_index:?}: check_durability(memo = {memo:#?}, last_changed={:?} <= verified_at={:?}) = {:?}",
            last_changed,
            verified_at,
            last_changed <= verified_at,
            memo = memo.tracing_debug()
        );
        if last_changed <= verified_at {
            // No input of the suitable durability has changed since last verified.
            ShallowUpdate::HigherDurability
        } else {
            ShallowUpdate::No
        }
    }

    #[inline]
    pub(super) fn update_shallow(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<'_, C>,
        update: ShallowUpdate,
    ) {
        if let ShallowUpdate::HigherDurability = update {
            memo.mark_as_verified(zalsa, database_key_index);
            memo.mark_outputs_as_verified(zalsa, database_key_index);
        }
    }

    /// Validates this memo if it is a provisional memo. Returns true for:
    /// * non provisional memos
    /// * provisional memos that have been successfully marked as verified final, that is, its
    ///   cycle heads have all been finalized.
    /// * provisional memos that have been created in the same revision and iteration and are part of the same cycle.
    ///   This check is skipped if `allow_non_finalized` is `false` as the memo itself is still not finalized. It's a provisional value.
    #[inline]
    pub(super) fn validate_may_be_provisional(
        &self,
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<'_, C>,
        allow_non_finalized: bool,
    ) -> bool {
        !memo.may_be_provisional()
            || self.validate_provisional(zalsa, database_key_index, memo)
            || (allow_non_finalized
                && self.validate_same_iteration(zalsa, zalsa_local, database_key_index, memo))
    }

    /// Check if this memo's cycle heads have all been finalized. If so, mark it verified final and
    /// return true, if not return false.
    #[inline]
    fn validate_provisional(
        &self,
        zalsa: &Zalsa,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<'_, C>,
    ) -> bool {
        crate::tracing::trace!(
            "{database_key_index:?}: validate_provisional(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );
        for cycle_head in memo.revisions.cycle_heads() {
            // Test if our cycle heads (with the same revision) are now finalized.
            let Some(kind) = zalsa
                .lookup_ingredient(cycle_head.database_key_index.ingredient_index())
                .provisional_status(zalsa, cycle_head.database_key_index.key_index())
            else {
                return false;
            };
            match kind {
                ProvisionalStatus::Provisional { .. } => return false,
                ProvisionalStatus::Final { iteration } => {
                    // It's important to also account for the revision for the case where:
                    // thread 1: `b` -> `a` (but only in the first iteration)
                    //               -> `c` -> `b`
                    // thread 2: `a` -> `b`
                    //
                    // If we don't account for the revision, then `a` (from iteration 0) will be finalized
                    // because its cycle head `b` is now finalized, but `b` never pulled `a` in the last iteration.
                    if iteration != cycle_head.iteration_count {
                        return false;
                    }

                    // FIXME: We can ignore this, I just don't have a use-case for this.
                    if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                        panic!("cannot mix `cycle_fn` and `cycle_result` in cycles")
                    }
                }
                ProvisionalStatus::FallbackImmediate => match C::CYCLE_STRATEGY {
                    CycleRecoveryStrategy::Panic => {
                        // Queries without fallback are not considered when inside a cycle.
                        return false;
                    }
                    // FIXME: We can do the same as with `CycleRecoveryStrategy::Panic` here, I just don't have
                    // a use-case for this.
                    CycleRecoveryStrategy::Fixpoint => {
                        panic!("cannot mix `cycle_fn` and `cycle_result` in cycles")
                    }
                    CycleRecoveryStrategy::FallbackImmediate => {}
                },
            }
        }
        // Relaxed is sufficient here because there are no other writes we need to ensure have
        // happened before marking this memo as verified-final.
        memo.revisions.verified_final.store(true, Ordering::Relaxed);
        true
    }

    /// If this is a provisional memo, validate that it was cached in the same iteration of the
    /// same cycle(s) that we are still executing. If so, it is valid for reuse. This avoids
    /// runaway re-execution of the same queries within a fixpoint iteration.
    fn validate_same_iteration(
        &self,
        zalsa: &Zalsa,
        zalsa_local: &ZalsaLocal,
        database_key_index: DatabaseKeyIndex,
        memo: &Memo<'_, C>,
    ) -> bool {
        crate::tracing::trace!(
            "{database_key_index:?}: validate_same_iteration(memo = {memo:#?})",
            memo = memo.tracing_debug()
        );

        let cycle_heads = memo.revisions.cycle_heads();
        if cycle_heads.is_empty() {
            return true;
        }

        // SAFETY: We do not access the query stack reentrantly.
        unsafe {
            zalsa_local.with_query_stack_unchecked(|stack| {
                cycle_heads.iter().all(|cycle_head| {
                    stack
                        .iter()
                        .rev()
                        .find(|query| query.database_key_index == cycle_head.database_key_index)
                        .map(|query| query.iteration_count())
                        .or_else(|| {
                            // If the cycle head isn't on our stack because:
                            //
                            // * another thread holds the lock on the cycle head (but it waits for the current query to complete)
                            // * we're in `maybe_changed_after` because `maybe_changed_after` doesn't modify the cycle stack
                            //
                            // check if the latest memo has the same iteration count.

                            // However, we've to be careful to skip over fixpoint initial values:
                            // If the head is the memo we're trying to validate, always return `None`
                            // to force a re-execution of the query. This is necessary because the query
                            // has obviously not completed its iteration yet.
                            //
                            // This should be rare but the `cycle_panic` test fails on some platforms (mainly GitHub actions)
                            // without this check. What happens there is that:
                            //
                            // * query a blocks on query b
                            // * query b tries to claim a, fails to do so and inserts the fixpoint initial value
                            // * query b completes and has `a` as head. It returns its query result Salsa blocks query b from
                            //   exiting inside `block_on` (or the thread would complete before the cycle iteration is complete)
                            // * query a resumes but panics because of the fixpoint iteration function
                            // * query b resumes. It rexecutes its own query which then tries to fetch a (which depends on itself because it's a fixpoint initial value).
                            //   Without this check, `validate_same_iteration` would return `true` because the latest memo for `a` is the fixpoint initial value.
                            //   But it should return `false` so that query b's thread re-executes `a` (which then also causes the panic).
                            //
                            // That's why we always return `None` if the cycle head is the same as the current database key index.
                            if cycle_head.database_key_index == database_key_index {
                                return None;
                            }

                            let ingredient = zalsa.lookup_ingredient(
                                cycle_head.database_key_index.ingredient_index(),
                            );
                            let wait_result = ingredient
                                .wait_for(zalsa, cycle_head.database_key_index.key_index());

                            if !wait_result.is_cycle() {
                                return None;
                            }

                            let provisional_status = ingredient.provisional_status(
                                zalsa,
                                cycle_head.database_key_index.key_index(),
                            )?;
                            provisional_status.iteration()
                        })
                        == Some(cycle_head.iteration_count)
                })
            })
        }
    }

    /// VerifyResult::Unchanged if the memo's value and `changed_at` time is up-to-date in the
    /// current revision. When this returns Unchanged with no cycle heads, it also updates the
    /// memo's `verified_at` field if needed to make future calls cheaper.
    ///
    /// Takes an [`ActiveQueryGuard`] argument because this function recursively
    /// walks dependencies of `old_memo` and may even execute them to see if their
    /// outputs have changed.
    ///
    /// `cycle_heads` tracks all cycle heads encountered while verifying if the memo has changed and that haven't been verified yet.
    /// These are all cycle heads from the entire subtree. Note, an empty set if the result is `Changed` doesn't suggest that there are no cycles.
    /// It only means that the verification didn't encounter any cycles during verification but the
    /// query might very well have participated in a cycle in its original computation.
    pub(super) fn deep_verify_memo(
        &self,
        db: &C::DbView,
        zalsa: &Zalsa,
        old_memo: &Memo<'_, C>,
        database_key_index: DatabaseKeyIndex,
        cycle_heads: &mut MaybeChangeAfterCycleHeads,
    ) -> VerifyResult {
        crate::tracing::debug!(
            "{database_key_index:?}: deep_verify_memo(old_memo = {old_memo:#?})",
            old_memo = old_memo.tracing_debug()
        );

        debug_assert!(!cycle_heads.contains(database_key_index));

        match old_memo.revisions.origin.as_ref() {
            QueryOriginRef::Derived(edges) => {
                let is_provisional = old_memo.may_be_provisional();

                // If the value is from the same revision but is still provisional, consider it changed
                // because we're now in a new iteration.
                if is_provisional
                    && self.shallow_verify_memo(zalsa, database_key_index, old_memo)
                        == ShallowUpdate::Verified
                {
                    return VerifyResult::changed();
                }

                #[cfg(feature = "accumulator")]
                let mut inputs = InputAccumulatedValues::Empty;
                let mut child_cycle_heads = CycleHeadKeys::new();

                // Fully tracked inputs? Iterate over the inputs and check them, one by one.
                //
                // NB: It's important here that we are iterating the inputs in the order that
                // they executed. It's possible that if the value of some input I0 is no longer
                // valid, then some later input I1 might never have executed at all, so verifying
                // it is still up to date is meaningless.
                for &edge in edges {
                    match edge.kind() {
                        QueryEdgeKind::Input(dependency_index) => {
                            debug_assert!(child_cycle_heads.is_empty());

                            // Pass fresh cycle heads to the child query to avoid
                            // that cycles introduced by siblings prevent queries from being verified.
                            let mut inner_cycle_heads = MaybeChangeAfterCycleHeads {
                                heads: std::mem::take(&mut child_cycle_heads),
                                has_outer_cycles: cycle_heads.has_any(),
                            };

                            let input_result = dependency_index.maybe_changed_after(
                                db.into(),
                                zalsa,
                                old_memo.verified_at.load(),
                                &mut inner_cycle_heads,
                            );

                            child_cycle_heads = inner_cycle_heads.heads;
                            // Aggregate all cycle heads
                            cycle_heads.append(&mut child_cycle_heads);

                            match input_result {
                                VerifyResult::Changed => return VerifyResult::changed(),
                                #[cfg(feature = "accumulator")]
                                VerifyResult::Unchanged { accumulated } => {
                                    inputs |= accumulated;
                                }
                                #[cfg(not(feature = "accumulator"))]
                                VerifyResult::Unchanged { .. } => {}
                            }
                        }
                        QueryEdgeKind::Output(dependency_index) => {
                            // Subtle: Mark outputs as validated now, even though we may
                            // later find an input that requires us to re-execute the function.
                            // Even if it re-execute, the function will wind up writing the same value,
                            // since all prior inputs were green. It's important to do this during
                            // this loop, because it's possible that one of our input queries will
                            // re-execute and may read one of our earlier outputs
                            // (e.g., in a scenario where we do something like
                            // `e = Entity::new(..); query(e);` and `query` reads a field of `e`).
                            //
                            // NB. Accumulators are also outputs, but the above logic doesn't
                            // quite apply to them. Since multiple values are pushed, the first value
                            // may be unchanged, but later values could be different.
                            // In that case, however, the data accumulated
                            // by this function cannot be read until this function is marked green,
                            // so even if we mark them as valid here, the function will re-execute
                            // and overwrite the contents.
                            dependency_index.mark_validated_output(zalsa, database_key_index);
                        }
                    }
                }

                cycle_heads.remove(database_key_index);

                // Possible scenarios here:
                //
                // 1. Cycle heads is empty. We traversed our full dependency graph and neither hit any
                //    cycles, nor found any changed dependencies. We can mark our memo verified and
                //    return Unchanged with empty cycle heads.
                //
                // 2. Cycle heads is non-empty, and does not contain our own key index. We are part of
                //    a cycle, and since we don't know if some other cycle participant that hasn't been
                //    traversed yet (that is, some other dependency of the cycle head, which is only a
                //    dependency of ours via the cycle) might still have changed, we can't yet mark our
                //    memo verified. We can return a provisional Unchanged, with cycle heads.
                //
                // 3. Cycle heads is non-empty, and contains only our own key index. We are the head of
                //    a cycle, and we've now traversed the entire cycle and found no changes, but no
                //    other cycle participants were verified (they would have all hit case 2 above).
                //    Similar to `execute`, return unchanged and lazily verify the other cycle-participants
                //    when they're used next.
                //
                // 4. Cycle heads is non-empty, and contains our own key index as well as other key
                //    indices. We are the head of a cycle nested within another cycle. We can't mark
                //    our own memo verified (for the same reason as in case 2: the full outer cycle
                //    hasn't been validated unchanged yet). We return Unchanged, with ourself removed
                //    from cycle heads. We will handle our own memo (and the rest of our cycle) on a
                //    future iteration; first the outer cycle head needs to verify itself.

                // 1 and 3
                if !cycle_heads.has_own() {
                    old_memo.mark_as_verified(zalsa, database_key_index);
                    #[cfg(feature = "accumulator")]
                    old_memo.revisions.accumulated_inputs.store(inputs);

                    if is_provisional {
                        old_memo
                            .revisions
                            .verified_final
                            .store(true, Ordering::Relaxed);
                    }
                }

                VerifyResult::unchanged_with_accumulated(
                    #[cfg(feature = "accumulator")]
                    {
                        inputs
                    },
                )
            }

            QueryOriginRef::Assigned(_) => {
                // If the value was assigned by another query,
                // and that query were up-to-date,
                // then we would have updated the `verified_at` field already.
                // So the fact that we are here means that it was not specified
                // during this revision or is otherwise stale.
                //
                // Example of how this can happen:
                //
                // Conditionally specified queries
                // where the value is specified
                // in rev 1 but not in rev 2.
                VerifyResult::changed()
            }
            // Return `Unchanged` similar to the initial value that we insert
            // when we hit the cycle. Any dependencies accessed when creating the fixpoint initial
            // are tracked by the outer query. Nothing should have changed assuming that the
            // fixpoint initial function is deterministic.
            QueryOriginRef::FixpointInitial => {
                cycle_heads.insert(database_key_index);
                VerifyResult::unchanged()
            }
            QueryOriginRef::DerivedUntracked(_) => {
                // Untracked inputs? Have to assume that it changed.
                VerifyResult::changed()
            }
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub(super) enum ShallowUpdate {
    /// The memo is from this revision and has already been verified
    Verified,

    /// The revision for the memo's durability hasn't changed. It can be marked as verified
    /// in this revision.
    HigherDurability,

    /// The memo requires a deep verification.
    No,
}

impl ShallowUpdate {
    pub(super) fn yes(&self) -> bool {
        matches!(
            self,
            ShallowUpdate::Verified | ShallowUpdate::HigherDurability
        )
    }
}

#[derive(Debug, Default)]
pub struct MaybeChangeAfterCycleHeads {
    heads: CycleHeadKeys,
    has_outer_cycles: bool,
}

impl MaybeChangeAfterCycleHeads {
    #[inline]
    fn contains(&self, key: DatabaseKeyIndex) -> bool {
        self.heads.contains(&key)
    }

    fn insert(&mut self, key: DatabaseKeyIndex) {
        self.heads.insert(key);
    }

    fn remove(&mut self, key: DatabaseKeyIndex) {
        self.heads.remove(key);
    }

    fn append(&mut self, heads: &mut CycleHeadKeys) {
        self.heads.append(heads);
    }

    pub fn has_any(&self) -> bool {
        self.has_outer_cycles || !self.heads.is_empty()
    }

    fn has_own(&self) -> bool {
        !self.heads.is_empty()
    }

    fn has_outer(&self) -> bool {
        self.has_outer_cycles
    }
}
