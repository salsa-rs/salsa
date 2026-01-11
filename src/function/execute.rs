use smallvec::SmallVec;

use crate::active_query::CompletedQuery;
use crate::cycle::{CycleHeads, CycleRecoveryStrategy, IterationCount};
use crate::function::memo::Memo;
use crate::function::sync::ReleaseMode;
use crate::function::{ClaimGuard, Configuration, IngredientImpl};
use crate::ingredient::WaitForResult;
use crate::plumbing::ZalsaLocal;
use crate::sync::thread;
use crate::tracked_struct::Identity;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{ActiveQueryGuard, QueryRevisions};
use crate::{tracing, Cancelled, Cycle};
use crate::{DatabaseKeyIndex, Event, EventKind, Id};

impl<C> IngredientImpl<C>
where
    C: Configuration,
{
    /// Executes the query function for the given `active_query`. Creates and stores
    /// a new memo with the result, backdated if possible. Once this completes,
    /// the query will have been popped off the active query stack.
    ///
    /// # Parameters
    ///
    /// * `db`, the database.
    /// * `active_query`, the active stack frame for the query to execute.
    /// * `opt_old_memo`, the older memo, if any existed. Used for backdating.
    ///
    /// # Returns
    /// The newly computed memo or `None` if this query is part of a larger cycle
    /// and `execute` blocked on a cycle head running on another thread. In this case,
    /// the memo is potentially outdated and needs to be refetched.
    #[inline(never)]
    pub(super) fn execute<'db>(
        &'db self,
        db: &'db C::DbView,
        mut claim_guard: ClaimGuard<'db>,
        zalsa_local: &'db ZalsaLocal,
        opt_old_memo: Option<&Memo<'db, C>>,
    ) -> Option<&'db Memo<'db, C>> {
        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();

        let id = database_key_index.key_index();
        let memo_ingredient_index = self.memo_ingredient_index(zalsa, id);

        crate::tracing::info!("{:?}: executing query", database_key_index);

        zalsa.event(&|| {
            Event::new(EventKind::WillExecute {
                database_key: database_key_index,
            })
        });

        let (new_value, mut completed_query) = match C::CYCLE_STRATEGY {
            CycleRecoveryStrategy::Panic => {
                let (new_value, active_query) = Self::execute_query(
                    db,
                    zalsa,
                    zalsa_local.push_query(database_key_index, IterationCount::initial()),
                    opt_old_memo,
                );
                (new_value, active_query.pop())
            }
            CycleRecoveryStrategy::FallbackImmediate | CycleRecoveryStrategy::Fixpoint => self
                .execute_maybe_iterate(
                    db,
                    opt_old_memo,
                    &mut claim_guard,
                    zalsa_local,
                    memo_ingredient_index,
                ),
        };

        if let Some(old_memo) = opt_old_memo {
            // If the new value is equal to the old one, then it didn't
            // really change, even if some of its inputs have. So we can
            // "backdate" its `changed_at` revision to be the same as the
            // old value.
            self.backdate_if_appropriate(
                old_memo,
                database_key_index,
                &mut completed_query.revisions,
                &new_value,
            );

            // Diff the new outputs with the old, to discard any no-longer-emitted
            // outputs and update the tracked struct IDs for seeding the next revision.
            self.diff_outputs(zalsa, database_key_index, old_memo, &completed_query);
        }

        let memo = self.insert_memo(
            zalsa,
            id,
            Memo::new(
                Some(new_value),
                zalsa.current_revision(),
                completed_query.revisions,
            ),
            memo_ingredient_index,
        );

        if claim_guard.drop() {
            None
        } else {
            Some(memo)
        }
    }

    fn execute_maybe_iterate<'db>(
        &'db self,
        db: &'db C::DbView,
        opt_old_memo: Option<&Memo<'db, C>>,
        claim_guard: &mut ClaimGuard<'db>,
        zalsa_local: &'db ZalsaLocal,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> (C::Output<'db>, CompletedQuery) {
        claim_guard.set_release_mode(ReleaseMode::Default);

        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();

        let id = database_key_index.key_index();

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // This is different from `opt_old_memo` which might be from a different revision.
        let mut last_provisional_memo_opt: Option<&Memo<'db, C>> = None;

        // TODO: Can we seed those somehow?
        let mut last_stale_tracked_ids: Vec<(Identity, Id)> = Vec::new();
        let mut iteration_count = IterationCount::initial();

        if let Some(old_memo) = opt_old_memo {
            if old_memo.verified_at.load() == zalsa.current_revision() {
                // The `DependencyGraph` locking propagates panics when another thread is blocked on a panicking query.
                // However, the locking doesn't handle the case where a thread fetches the result of a panicking
                // cycle head query **after** all locks were released. That's what we do here.
                // We could consider re-executing the entire cycle but:
                // a) It's tricky to ensure that all queries participating in the cycle will re-execute
                //    (we can't rely on `iteration_count` being updated for nested cycles because the nested cycles may have completed successfully).
                // b) It's guaranteed that this query will panic again anyway.
                // That's why we simply propagate the panic here. It simplifies our lives and it also avoids duplicate panic messages.
                if old_memo.value.is_none() {
                    tracing::warn!("Propagating panic for cycle head that panicked in an earlier execution in that revision");
                    Cancelled::PropagatedPanic.throw();
                }

                // Only use the last provisional memo if it was a cycle head in the last iteration. This is to
                // force at least two executions.
                if old_memo.cycle_heads().contains(&database_key_index) {
                    last_provisional_memo_opt = Some(old_memo);
                }

                iteration_count = old_memo.revisions.iteration();
            }
        }

        let _poison_guard =
            PoisonProvisionalIfPanicking::new(self, zalsa, id, memo_ingredient_index);

        let (new_value, completed_query) = loop {
            let active_query = zalsa_local.push_query(database_key_index, iteration_count);

            // Tracked struct ids that existed in the previous revision
            // but weren't recreated in the last iteration. It's important that we seed the next
            // query with these ids because the query might re-create them as part of the next iteration.
            // This is not only important to ensure that the re-created tracked structs have the same ids,
            // it's also important to ensure that these tracked structs get removed
            // if they aren't recreated when reaching the final iteration.
            active_query.seed_tracked_struct_ids(&last_stale_tracked_ids);

            let (mut new_value, mut active_query) = Self::execute_query(
                db,
                zalsa,
                active_query,
                last_provisional_memo_opt.or(opt_old_memo),
            );

            // Take the cycle heads to not-fight-rust's-borrow-checker.
            let mut cycle_heads = active_query.take_cycle_heads();

            // If there are no cycle heads, break out of the loop.
            if cycle_heads.is_empty() {
                iteration_count = iteration_count.increment().unwrap_or_else(|| {
                    tracing::warn!("{database_key_index:?}: execute: too many cycle iterations");
                    panic!("{database_key_index:?}: execute: too many cycle iterations")
                });

                let mut completed_query = active_query.pop();
                completed_query
                    .revisions
                    .update_cycle_participant_iteration_count(iteration_count);

                claim_guard.set_release_mode(ReleaseMode::SelfOnly);
                break (new_value, completed_query);
            }

            let (max_iteration_count, depends_on_self) = collect_all_cycle_heads(
                zalsa,
                &mut cycle_heads,
                database_key_index,
                iteration_count,
            );

            let outer_cycle = outer_cycle(zalsa, zalsa_local, &cycle_heads, database_key_index);

            // Did the new result we got depend on our own provisional value, in a cycle?
            // If not, return because this query is not a cycle head.
            if !depends_on_self {
                let Some(outer_cycle) = outer_cycle else {
                    panic!("cycle participant with non-empty cycle heads and that doesn't depend on itself must have an outer cycle responsible to finalize the query later (query: {database_key_index:?}, cycle heads: {cycle_heads:?}).");
                };

                // For FallbackImmediate, use the fallback value instead of the computed value
                // for all cycle participants. This ensures that the results don't depend on the query call order, see
                // https://github.com/salsa-rs/salsa/pull/798#issuecomment-2812855285.
                let new_value = if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                    C::cycle_initial(db, id, C::id_to_input(zalsa, id))
                } else {
                    new_value
                };

                let completed_query = complete_cycle_participant(
                    active_query,
                    claim_guard,
                    cycle_heads,
                    outer_cycle,
                    iteration_count,
                );

                break (new_value, completed_query);
            }

            // Get the last provisional value for this query so that we can compare it with the new value
            // to test if the cycle converged.
            let last_provisional_memo = last_provisional_memo_opt.unwrap_or_else(|| {
                // This is our first time around the loop; a provisional value must have been
                // inserted into the memo table when the cycle was hit, so let's pull our
                // initial provisional value from there.
                let memo = self
                    .get_memo_from_table_for(zalsa, id, memo_ingredient_index)
                    .unwrap_or_else(|| {
                        unreachable!(
                            "{database_key_index:#?} is a cycle head, \
                                        but no provisional memo found"
                        )
                    });

                debug_assert!(memo.may_be_provisional());
                memo
            });

            let last_provisional_value = last_provisional_memo.value.as_ref();

            let last_provisional_value = last_provisional_value.expect(
                "`fetch_cold_cycle` should have inserted a provisional memo with Cycle::initial",
            );
            tracing::debug!(
                "{database_key_index:?}: execute: \
                I am a cycle head, comparing last provisional value with new value"
            );

            // If this is the outermost cycle, use the maximum iteration count of all cycles.
            // This is important for when later iterations introduce new cycle heads (that then
            // become the outermost cycle). We want to ensure that the iteration count keeps increasing
            // for all queries or they won't be re-executed because `validate_same_iteration` would
            // pass when we go from 1 -> 0 and then increment by 1 to 1).
            iteration_count = if outer_cycle.is_none() {
                max_iteration_count
            } else {
                // Otherwise keep the iteration count because outer cycles
                // already have a cycle head with this exact iteration count (and we don't allow
                // heads from different iterations).
                iteration_count
            };

            // For FallbackImmediate, the value always converges immediately (we use the
            // fallback directly). We still iterate if metadata hasn't converged.
            // For Fixpoint, ask the recovery function what value to use and check convergence.
            let value_converged = if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                // Use the fallback value instead of the computed value.
                new_value = C::cycle_initial(db, id, C::id_to_input(zalsa, id));
                true
            } else {
                let cycle = Cycle {
                    head_ids: cycle_heads.ids(),
                    id,
                    iteration: iteration_count.as_u32(),
                };
                // We are in a cycle that hasn't converged; ask the user's
                // cycle-recovery function what to do (it may return the same value or a different one):
                new_value = C::recover_from_cycle(
                    db,
                    &cycle,
                    last_provisional_value,
                    new_value,
                    C::id_to_input(zalsa, id),
                );

                C::values_equal(&new_value, last_provisional_value)
            };

            let new_cycle_heads = active_query.take_cycle_heads();
            assert_no_new_cycle_heads(&cycle_heads, new_cycle_heads, database_key_index);

            let completed_query = match try_complete_cycle_head(
                active_query,
                claim_guard,
                cycle_heads,
                &last_provisional_memo.revisions,
                outer_cycle,
                iteration_count,
                value_converged,
            ) {
                Ok(completed_query) => {
                    break (new_value, completed_query);
                }
                Err((completed_query, new_iteration_count)) => {
                    iteration_count = new_iteration_count;
                    completed_query
                }
            };

            let new_memo = self.insert_memo(
                zalsa,
                id,
                Memo::new(
                    Some(new_value),
                    zalsa.current_revision(),
                    completed_query.revisions,
                ),
                memo_ingredient_index,
            );

            last_provisional_memo_opt = Some(new_memo);

            last_stale_tracked_ids = completed_query.stale_tracked_structs;

            continue;
        };

        tracing::debug!(
            "{database_key_index:?}: execute_maybe_iterate: result.revisions = {revisions:#?}",
            revisions = &completed_query.revisions
        );

        (new_value, completed_query)
    }

    #[inline]
    fn execute_query<'db>(
        db: &'db C::DbView,
        zalsa: &'db Zalsa,
        active_query: ActiveQueryGuard<'db>,
        opt_old_memo: Option<&Memo<'db, C>>,
    ) -> (C::Output<'db>, ActiveQueryGuard<'db>) {
        if let Some(old_memo) = opt_old_memo {
            // If we already executed this query once, then use the tracked-struct ids from the
            // previous execution as the starting point for the new one.
            active_query.seed_tracked_struct_ids(old_memo.revisions.tracked_struct_ids());

            // Copy over all inputs and outputs from a previous iteration.
            // This is necessary to:
            // * ensure that tracked struct created during the previous iteration
            //   (and are owned by the query) are alive even if the query in this iteration no longer creates them.
            // * ensure the final returned memo depends on all inputs from all iterations.
            if old_memo.may_be_provisional()
                && old_memo.verified_at.load() == zalsa.current_revision()
            {
                active_query.seed_iteration(&old_memo.revisions);
            }
        }

        // Query was not previously executed, or value is potentially
        // stale, or value is absent. Let's execute!
        let new_value = C::execute(
            db,
            C::id_to_input(zalsa, active_query.database_key_index.key_index()),
        );

        (new_value, active_query)
    }
}

/// Replaces any inserted memo with a fixpoint initial memo without a value if the current thread panics.
///
/// A regular query doesn't insert any memo if it panics and the query
/// simply gets re-executed if any later called query depends on the panicked query (and will panic again unless the query isn't deterministic).
///
/// Unfortunately, this isn't the case for cycle heads because Salsa first inserts the fixpoint initial memo and later inserts
/// provisional memos for every iteration. Detecting whether a query has previously panicked
/// in `fetch` (e.g., `validate_same_iteration`) and requires re-execution is probably possible but not very straightforward
/// and it's easy to get it wrong, which results in infinite loops where `Memo::provisional_retry` keeps retrying to get the latest `Memo`
/// but `fetch` doesn't re-execute the query for reasons.
///
/// Specifically, a Memo can linger after a panic, which is then incorrectly returned
/// by `fetch_cold_cycle` because it passes the `shallow_verified_memo` check instead of inserting
/// a new fix point initial value if that happens.
///
/// We could insert a fixpoint initial value here, but it seems unnecessary.
struct PoisonProvisionalIfPanicking<'a, C: Configuration> {
    ingredient: &'a IngredientImpl<C>,
    zalsa: &'a Zalsa,
    id: Id,
    memo_ingredient_index: MemoIngredientIndex,
}

impl<'a, C: Configuration> PoisonProvisionalIfPanicking<'a, C> {
    fn new(
        ingredient: &'a IngredientImpl<C>,
        zalsa: &'a Zalsa,
        id: Id,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> Self {
        Self {
            ingredient,
            zalsa,
            id,
            memo_ingredient_index,
        }
    }
}

impl<C: Configuration> Drop for PoisonProvisionalIfPanicking<'_, C> {
    fn drop(&mut self) {
        if thread::panicking() {
            let revisions = QueryRevisions::fixpoint_initial(
                self.ingredient.database_key_index(self.id),
                IterationCount::initial(),
            );

            let memo = Memo::new(None, self.zalsa.current_revision(), revisions);
            self.ingredient
                .insert_memo(self.zalsa, self.id, memo, self.memo_ingredient_index);
        }
    }
}

/// Returns the key of any potential outer cycle head or `None` if there is no outer cycle.
///
/// That is, any query that's currently blocked on the result computed by this query (claiming it results in a cycle).
fn outer_cycle(
    zalsa: &Zalsa,
    zalsa_local: &ZalsaLocal,
    cycle_heads: &CycleHeads,
    current_key: DatabaseKeyIndex,
) -> Option<DatabaseKeyIndex> {
    // First, look for the outer most cycle head on the same thread.
    // Using the outer most over the inner most should reduce the need
    // for transitive transfers.
    // SAFETY: We don't call into with_query_stack recursively
    if let Some(same_thread) = unsafe {
        zalsa_local.with_query_stack_unchecked(|stack| {
            stack
                .iter()
                .find(|active_query| {
                    active_query.database_key_index != current_key
                        && cycle_heads.contains(&active_query.database_key_index)
                })
                .map(|active_query| active_query.database_key_index)
        })
    } {
        return Some(same_thread);
    }

    // Check for any outer cycle head running on a different thread.
    cycle_heads
        .iter_not_eq(current_key)
        .rfind(|head| {
            let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

            matches!(
                ingredient.wait_for(zalsa, head.database_key_index.key_index()),
                WaitForResult::Cycle { inner: false }
            )
        })
        .map(|head| head.database_key_index)
}

/// Ensure that we resolve the latest cycle heads from any provisional value this query depended on during execution.
///
/// ```txt
/// E -> C -> D -> B -> A -> B (cycle)
///                     -- A completes, heads = [B]
/// E -> C -> D -> B -> C (cycle)
///                  -> D (cycle)
///                -- B completes, heads = [B, C, D]
/// E -> C -> D -> E (cycle)
///           -- D completes, heads = [E, B, C, D]
/// E -> C
///      -- C completes, heads = [E, B, C, D]
/// E -> X -> A
///      -- X completes, heads = [B]
/// ```
///
/// Note how `X` only depends on `A`. It doesn't know that it's part of the outer cycle `X`.
/// An old implementation resolved the cycle heads 1-level deep but that's not enough, because
/// `X` then completes with `[B, C, D]` as it's heads. But `B`, `C`, and `D` are no longer on the stack
/// when `X` completes (which is the real outermost cycle). That's why we need to resolve all cycle heads
/// recursively, so that `X` completes with `[B, C, D, E]
fn collect_all_cycle_heads(
    zalsa: &Zalsa,
    cycle_heads: &mut CycleHeads,
    database_key_index: DatabaseKeyIndex,
    iteration_count: IterationCount,
) -> (IterationCount, bool) {
    fn collect_recursive(
        zalsa: &Zalsa,
        current_head: DatabaseKeyIndex,
        me: DatabaseKeyIndex,
        query_heads: &CycleHeads,
        missing_heads: &mut SmallVec<[(DatabaseKeyIndex, IterationCount); 4]>,
    ) -> (IterationCount, bool) {
        if current_head == me {
            return (IterationCount::initial(), true);
        }

        let mut max_iteration_count = IterationCount::initial();
        let mut depends_on_self = false;

        let ingredient = zalsa.lookup_ingredient(current_head.ingredient_index());

        let provisional_status = ingredient
            .provisional_status(zalsa, current_head.key_index())
            .expect("cycle head memo must have been created during the execution");

        // A query should only ever depend on other heads that are provisional.
        // If this invariant is violated, it means that this query participates in a cycle,
        // but it wasn't executed in the last iteration of said cycle.
        assert!(provisional_status.is_provisional());

        for head in provisional_status.cycle_heads() {
            let iteration_count = head.iteration_count.load();
            max_iteration_count = max_iteration_count.max(iteration_count);

            if query_heads.contains(&head.database_key_index) {
                continue;
            }

            let head_as_tuple = (head.database_key_index, iteration_count);

            if missing_heads.contains(&head_as_tuple) {
                continue;
            }

            missing_heads.push((head.database_key_index, iteration_count));

            let (nested_max_iteration_count, nested_depends_on_self) = collect_recursive(
                zalsa,
                head.database_key_index,
                me,
                query_heads,
                missing_heads,
            );

            max_iteration_count = max_iteration_count.max(nested_max_iteration_count);
            depends_on_self |= nested_depends_on_self;
        }

        (max_iteration_count, depends_on_self)
    }

    let mut missing_heads: SmallVec<[(DatabaseKeyIndex, IterationCount); 4]> = SmallVec::new();
    let mut max_iteration_count = iteration_count;
    let mut depends_on_self = false;

    for head in &*cycle_heads {
        let (recursive_max_iteration, recursive_depends_on_self) = collect_recursive(
            zalsa,
            head.database_key_index,
            database_key_index,
            cycle_heads,
            &mut missing_heads,
        );

        max_iteration_count = max_iteration_count.max(recursive_max_iteration);
        depends_on_self |= recursive_depends_on_self;
    }

    for (head, iteration) in missing_heads {
        cycle_heads.insert(head, iteration);
    }

    (max_iteration_count, depends_on_self)
}

fn complete_cycle_participant(
    active_query: ActiveQueryGuard,
    claim_guard: &mut ClaimGuard,
    cycle_heads: CycleHeads,
    outer_cycle: DatabaseKeyIndex,
    iteration_count: IterationCount,
) -> CompletedQuery {
    // For as long as this query participates in any cycle, don't release its lock, instead
    // transfer it to the outermost cycle head. This prevents any other thread
    // from claiming this query (all cycle heads are potential entry points to the same cycle),
    // which would result in them competing for the same locks (we want the locks to converge to a single cycle head).
    claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));

    let database_key_index = active_query.database_key_index;
    let mut completed_query = active_query.pop();
    *completed_query.revisions.verified_final.get_mut() = false;
    completed_query.revisions.set_cycle_heads(cycle_heads);

    let iteration_count = iteration_count.increment().unwrap_or_else(|| {
        tracing::warn!("{database_key_index:?}: execute: too many cycle iterations");
        panic!("{database_key_index:?}: execute: too many cycle iterations")
    });

    // The outermost query only bumps the iteration count of cycle heads. It doesn't
    // increment the iteration count for cycle participants. It's important that we bump the
    // iteration count here or the head will re-use the same iteration count in the next
    // iteration (which can break cache invalidation).
    completed_query
        .revisions
        .update_cycle_participant_iteration_count(iteration_count);

    completed_query
}

/// Tries to complete the cycle head if it has converged.
///
/// Returns `Ok` if the cycle head has converged or if it is part of an outer cycle.
/// Returns `Err` if the cycle head needs to keep iterating.
fn try_complete_cycle_head(
    active_query: ActiveQueryGuard,
    claim_guard: &mut ClaimGuard,
    cycle_heads: CycleHeads,
    last_provisional_revisions: &QueryRevisions,
    outer_cycle: Option<DatabaseKeyIndex>,
    iteration_count: IterationCount,
    value_converged: bool,
) -> Result<CompletedQuery, (CompletedQuery, IterationCount)> {
    let me = active_query.database_key_index;

    let mut completed_query = active_query.pop();

    // It's important to force a re-execution of the cycle if `changed_at` or `durability` has changed
    // to ensure the reduced durability and changed propagates to all queries depending on this head.
    let metadata_converged = last_provisional_revisions.durability
        == completed_query.revisions.durability
        && last_provisional_revisions.changed_at == completed_query.revisions.changed_at
        && last_provisional_revisions.origin.is_derived_untracked()
            == completed_query.revisions.origin.is_derived_untracked();

    let this_converged = value_converged && metadata_converged;

    if let Some(outer_cycle) = outer_cycle {
        let me = claim_guard.database_key_index();

        tracing::info!(
            "Detected nested cycle {me:?}, iterate it as part of the outer cycle {outer_cycle:?}"
        );

        completed_query.revisions.set_cycle_heads(cycle_heads);
        // Store whether this cycle has converged, so that the outer cycle can check it.
        completed_query
            .revisions
            .set_cycle_converged(this_converged);
        *completed_query.revisions.verified_final.get_mut() = false;

        // Transfer ownership of this query to the outer cycle, so that it can claim it
        // and other threads don't compete for the same lock.
        claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));

        return Ok(completed_query);
    }

    let zalsa = claim_guard.zalsa();

    // If this is the outermost cycle, test if all inner cycles have converged as well.
    let converged = this_converged
        && cycle_heads.iter_not_eq(me).all(|head| {
            let database_key_index = head.database_key_index;
            let ingredient = zalsa.lookup_ingredient(database_key_index.ingredient_index());

            let converged = ingredient.cycle_converged(zalsa, database_key_index.key_index());

            if !converged {
                tracing::debug!("inner cycle {database_key_index:?} has not converged",);
            }

            converged
        });

    if converged {
        tracing::debug!(
            "{me:?}: execute: fixpoint iteration has a final value after {iteration_count:?} iterations"
        );

        // Set the nested cycles as verified. This is necessary because
        // `validate_provisional` doesn't follow cycle heads recursively (and the memos now depend on all cycle heads).
        for head in cycle_heads.iter_not_eq(me) {
            let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());
            ingredient.finalize_cycle_head(zalsa, head.database_key_index.key_index());
        }

        *completed_query.revisions.verified_final.get_mut() = true;

        zalsa.event(&|| {
            Event::new(EventKind::DidFinalizeCycle {
                database_key: me,
                iteration_count,
            })
        });

        return Ok(completed_query);
    }

    *completed_query.revisions.verified_final.get_mut() = false;

    // The fixpoint iteration hasn't converged. Iterate again...
    let iteration_count = iteration_count.increment().unwrap_or_else(|| {
        tracing::warn!("{me:?}: execute: too many cycle iterations");
        panic!("{me:?}: execute: too many cycle iterations")
    });

    zalsa.event(&|| {
        Event::new(EventKind::WillIterateCycle {
            database_key: me,
            iteration_count,
        })
    });

    tracing::info!("{me:?}: execute: iterate again ({iteration_count:?})...",);

    // Update the iteration count of nested cycles.
    for head in cycle_heads.iter_not_eq(me) {
        let ingredient = zalsa.lookup_ingredient(head.database_key_index.ingredient_index());

        ingredient.set_cycle_iteration_count(
            zalsa,
            head.database_key_index.key_index(),
            iteration_count,
        );
    }

    // Update the iteration count of this cycle head, but only after restoring
    // the cycle heads array (or this becomes a no-op).
    // We don't call the same method on `cycle_heads` because that one doens't update
    // the `memo.iteration_count`
    completed_query.revisions.set_cycle_heads(cycle_heads);
    completed_query
        .revisions
        .update_iteration_count_mut(me, iteration_count);

    Err((completed_query, iteration_count))
}

fn assert_no_new_cycle_heads(
    cycle_heads: &CycleHeads,
    new_cycle_heads: CycleHeads,
    me: DatabaseKeyIndex,
) {
    for head in new_cycle_heads {
        if !cycle_heads.contains(&head.database_key_index) {
            panic!("Cycle recovery function for {me:?} introduced a cycle, depending on {:?}. This is not allowed.", head.database_key_index);
        }
    }
}
