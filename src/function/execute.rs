use crate::active_query::CompletedQuery;
use crate::cycle::{CycleHeads, CycleRecoveryStrategy, IterationCount};
use crate::function::memo::Memo;
use crate::function::sync::ReleaseMode;
use crate::function::{ClaimGuard, Configuration, IngredientImpl};
use crate::hash::{FxHashSet, FxIndexSet};
use crate::ingredient::WaitForResult;
use crate::plumbing::ZalsaLocal;
use crate::sync::thread;
use crate::tracked_struct::Identity;
use crate::zalsa::{MemoIngredientIndex, Zalsa};
use crate::zalsa_local::{ActiveQueryGuard, QueryEdge, QueryEdgeKind, QueryRevisions};
use crate::{Cycle, tracing};
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
                    claim_guard
                        .zalsa_local()
                        .push_query(database_key_index, IterationCount::initial()),
                    opt_old_memo,
                );
                (new_value, active_query.pop())
            }
            CycleRecoveryStrategy::FallbackImmediate | CycleRecoveryStrategy::Fixpoint => {
                let zalsa_local = claim_guard.zalsa_local();
                let was_disabled = zalsa_local.set_cancellation_disabled(true);

                let res = self.execute_maybe_iterate(
                    db,
                    opt_old_memo,
                    &mut claim_guard,
                    memo_ingredient_index,
                );

                zalsa_local.set_cancellation_disabled(was_disabled);

                res
            }
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

        let mut memo = Memo::new(
            Some(new_value),
            zalsa.current_revision(),
            completed_query.revisions,
        );
        if memo.may_be_provisional()
            && let Some(active_cycle) = completed_query.active_cycle
        {
            memo = memo.with_active_cycle(zalsa, database_key_index, active_cycle);
        }

        let memo = self.insert_memo(zalsa, id, memo, memo_ingredient_index);

        if claim_guard.drop() { None } else { Some(memo) }
    }

    fn execute_maybe_iterate<'db>(
        &'db self,
        db: &'db C::DbView,
        opt_old_memo: Option<&Memo<'db, C>>,
        claim_guard: &mut ClaimGuard<'db>,
        memo_ingredient_index: MemoIngredientIndex,
    ) -> (C::Output<'db>, CompletedQuery) {
        claim_guard.set_release_mode(ReleaseMode::Default);

        let database_key_index = claim_guard.database_key_index();
        let zalsa = claim_guard.zalsa();

        let id = database_key_index.key_index();

        // Our provisional value from the previous iteration, when doing fixpoint iteration.
        // This is different from `opt_old_memo` which might be from a different revision.
        let mut last_provisional_memo_opt: Option<&Memo<'db, C>> = None;

        let mut last_stale_tracked_ids: Vec<(Identity, Id)> = Vec::new();
        let mut iteration_count = IterationCount::initial();
        let mut current_active_cycle = None;

        if let Some(old_memo) = opt_old_memo {
            if old_memo.verified_at.load() == zalsa.current_revision() {
                current_active_cycle = old_memo.revisions.active_cycle();

                if current_active_cycle.is_some_and(|active_cycle| {
                    zalsa
                        .active_cycles()
                        .contains_participant(active_cycle, database_key_index)
                }) {
                    last_provisional_memo_opt = Some(old_memo);
                }

                iteration_count = current_active_cycle
                    .and_then(|active_cycle| zalsa.active_cycles().iteration(active_cycle))
                    .unwrap_or_else(IterationCount::initial);
            }
        }

        let _poison_guard =
            PoisonProvisionalIfPanicking::new(self, zalsa, id, memo_ingredient_index);

        let (new_value, completed_query) = loop {
            let active_query = claim_guard
                .zalsa_local()
                .push_query(database_key_index, iteration_count);

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
            let cycle_heads = active_query.take_cycle_heads();

            // If there are no cycle heads, break out of the loop.
            if cycle_heads.is_empty() {
                break (new_value, active_query.pop());
            }

            let active_cycle = active_query
                .active_cycle()
                .or(current_active_cycle)
                .or_else(|| {
                    cycle_heads
                        .iter()
                        .find_map(|head| zalsa.active_cycles().key_for(head.database_key_index))
                });
            current_active_cycle = active_cycle;
            if let Some(active_cycle) = active_cycle {
                for head in &cycle_heads {
                    if let Some(head_cycle) = zalsa.active_cycles().key_for(head.database_key_index)
                        && head_cycle != active_cycle
                    {
                        zalsa.active_cycles().merge(active_cycle, head_cycle);
                    }
                    zalsa
                        .active_cycles()
                        .add_head(active_cycle, head.database_key_index);
                }
                if let Some(active_iteration) = zalsa.active_cycles().iteration(active_cycle) {
                    iteration_count = active_iteration;
                }
            }

            let depends_on_self = cycle_heads.contains(&database_key_index);

            let local_outer_cycle = outer_cycle(
                zalsa,
                claim_guard.zalsa_local(),
                &cycle_heads,
                database_key_index,
            );

            // Did the new result we got depend on our own provisional value, in a cycle?
            // If not, return because this query is not a cycle head.
            if !depends_on_self {
                let outer_cycle = local_outer_cycle.or_else(|| {
                    let active_cycle_heads = zalsa.active_cycles().current_heads(active_cycle?)?;
                    outer_cycle(
                        zalsa,
                        claim_guard.zalsa_local(),
                        &active_cycle_heads,
                        database_key_index,
                    )
                });
                // For FallbackImmediate, use the fallback value instead of the computed value
                // for all cycle participants. This ensures that the results don't depend on the query call order, see
                // https://github.com/salsa-rs/salsa/pull/798#issuecomment-2812855285.
                let new_value = if C::CYCLE_STRATEGY == CycleRecoveryStrategy::FallbackImmediate {
                    C::cycle_initial(db, id, C::id_to_input(zalsa, id))
                } else {
                    new_value
                };

                let completed_query =
                    complete_cycle_participant(active_query, claim_guard, cycle_heads, outer_cycle);

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
                local_outer_cycle,
                iteration_count,
                value_converged,
            ) {
                CycleHeadCompletion::Complete(completed_query) => {
                    break (new_value, completed_query);
                }
                CycleHeadCompletion::Iterate(completed_query, new_iteration_count) => {
                    iteration_count = new_iteration_count;
                    completed_query
                }
            };

            let mut memo = Memo::new(
                Some(new_value),
                zalsa.current_revision(),
                completed_query.revisions,
            );
            if memo.may_be_provisional()
                && let Some(active_cycle) = completed_query.active_cycle
            {
                memo = memo.with_active_cycle(zalsa, database_key_index, active_cycle);
            }
            let new_memo = self.insert_memo(zalsa, id, memo, memo_ingredient_index);

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

/// Removes central cycle state if the current thread panics.
///
/// A regular query doesn't insert any memo if it panics and the query
/// simply gets re-executed if any later called query depends on the panicked query (and will panic again unless the query isn't deterministic).
///
/// Cycle heads insert provisional memos before the cycle finishes. If the query panics,
/// removing the central state makes those provisional memos fail the same-iteration lookup
/// and forces a later read to re-execute them.
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
            let active_cycle = self
                .ingredient
                .get_memo_from_table_for(self.zalsa, self.id, self.memo_ingredient_index)
                .and_then(|memo| memo.revisions.active_cycle())
                .or_else(|| {
                    self.zalsa
                        .active_cycles()
                        .key_for(self.ingredient.database_key_index(self.id))
                });
            if let Some(active_cycle) = active_cycle {
                self.zalsa.active_cycles().remove(active_cycle);
            }
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

// Called when completing the query of a cycle head participating
// in an outer cycle head (which doesn't depend on itself).
fn complete_cycle_participant(
    active_query: ActiveQueryGuard,
    claim_guard: &mut ClaimGuard,
    cycle_heads: CycleHeads,
    outer_cycle: Option<DatabaseKeyIndex>,
) -> CompletedQuery {
    // Keep the participant locked by a proven outer owner when one exists. If the
    // active cycle no longer has a blocking owner, releasing the lock is safe: the
    // central cycle state still makes the provisional memo stale on the next read.
    if let Some(outer_cycle) = outer_cycle {
        claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));
    }
    let zalsa = claim_guard.zalsa();

    let database_key_index = active_query.database_key_index;
    let mut completed_query = active_query.pop();

    flatten_cycle_dependencies(zalsa, &mut completed_query.revisions);

    *completed_query.revisions.verified_final.get_mut() = false;
    completed_query.cycle_heads = cycle_heads;
    completed_query.active_cycle = completed_query
        .cycle_heads
        .iter()
        .find_map(|head| zalsa.active_cycles().key_for(head.database_key_index))
        .or_else(|| zalsa.active_cycles().key_for(database_key_index))
        .or_else(|| outer_cycle.and_then(|outer_cycle| zalsa.active_cycles().key_for(outer_cycle)));

    completed_query
}

enum CycleHeadCompletion {
    Complete(CompletedQuery),
    Iterate(CompletedQuery, IterationCount),
}

/// Tries to complete the cycle head if it has converged.
fn try_complete_cycle_head(
    active_query: ActiveQueryGuard,
    claim_guard: &mut ClaimGuard,
    cycle_heads: CycleHeads,
    last_provisional_revisions: &QueryRevisions,
    outer_cycle: Option<DatabaseKeyIndex>,
    iteration_count: IterationCount,
    value_converged: bool,
) -> CycleHeadCompletion {
    let me = active_query.database_key_index;
    let zalsa = claim_guard.zalsa();
    let active_cycle = zalsa.active_cycles().key_for(me);

    let mut completed_query = active_query.pop();
    flatten_cycle_dependencies(zalsa, &mut completed_query.revisions);

    // It's important to force a re-execution of the cycle if `changed_at` or `durability` has changed
    // to ensure the reduced durability and changed propagates to all queries depending on this head.
    let metadata_converged = last_provisional_revisions.durability
        == completed_query.revisions.durability
        && last_provisional_revisions.changed_at == completed_query.revisions.changed_at
        && last_provisional_revisions.origin.is_derived_untracked()
            == completed_query.revisions.origin.is_derived_untracked();

    let this_converged = value_converged && metadata_converged;

    if let Some(outer_cycle) = outer_cycle {
        tracing::info!(
            "Detected nested cycle {me:?}, iterate it as part of the outer cycle {outer_cycle:?}"
        );

        *completed_query.revisions.verified_final.get_mut() = false;
        completed_query.cycle_heads = cycle_heads;
        completed_query.active_cycle = active_cycle;
        if let Some(active_cycle) = active_cycle {
            zalsa
                .active_cycles()
                .set_converged(active_cycle, this_converged);
        }

        // Transfer ownership of this query to the outer cycle, so that it can claim it
        // and other threads don't compete for the same lock.
        claim_guard.set_release_mode(ReleaseMode::TransferTo(outer_cycle));

        return CycleHeadCompletion::Complete(completed_query);
    }

    // This is the outermost cycle, drive the cycle forward. Nested heads record
    // their convergence directly on the shared active cycle state.
    let inner_cycles_converged = active_cycle
        .and_then(|active_cycle| zalsa.active_cycles().converged(active_cycle))
        .unwrap_or(true);

    let converged = this_converged && inner_cycles_converged;

    if converged {
        tracing::debug!(
            "{me:?}: execute: fixpoint iteration has a final value after {iteration_count:?} iterations"
        );

        if let Some(active_cycle) = active_cycle {
            let completed_whole_cycle = zalsa
                .active_cycles()
                .heads_are_covered_by(active_cycle, &cycle_heads)
                .unwrap_or(true);
            let memos = if completed_whole_cycle {
                zalsa.active_cycles().current_memo_keys(active_cycle)
            } else {
                zalsa
                    .active_cycles()
                    .take_memo_keys(active_cycle, &cycle_heads)
            };
            if let Some(memos) = memos {
                for memo in memos {
                    let ingredient = zalsa.lookup_ingredient(memo.ingredient_index());
                    ingredient.finalize_cycle_head(zalsa, memo.key_index());
                }
            }
            if completed_whole_cycle {
                zalsa.active_cycles().remove(active_cycle);
            }
        }
        completed_query.active_cycle = None;

        *completed_query.revisions.verified_final.get_mut() = true;

        zalsa.event(&|| {
            Event::new(EventKind::DidFinalizeCycle {
                database_key: me,
                iteration_count,
            })
        });

        return CycleHeadCompletion::Complete(completed_query);
    }

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

    if let Some(active_cycle) = active_cycle {
        zalsa
            .active_cycles()
            .start_next_iteration(active_cycle, iteration_count);
        zalsa.active_cycles().add_head(active_cycle, me);
    }

    *completed_query.revisions.verified_final.get_mut() = false;
    completed_query.cycle_heads = cycle_heads;
    completed_query.active_cycle = active_cycle;

    CycleHeadCompletion::Iterate(completed_query, iteration_count)
}

fn assert_no_new_cycle_heads(
    cycle_heads: &CycleHeads,
    new_cycle_heads: CycleHeads,
    me: DatabaseKeyIndex,
) {
    for head in new_cycle_heads {
        if !cycle_heads.contains(&head.database_key_index) {
            panic!(
                "Cycle recovery function for {me:?} introduced a cycle, depending on {:?}. This is not allowed.",
                head.database_key_index
            );
        }
    }
}

thread_local! {
    /// Pool the `seen` and `flattened` sets for reuse on the same thread.
    ///
    /// Benchmarks showed that repeatedly allocating and regrowing those sets is expensive.
    static FLATTEN_MAPS: std::cell::Cell<Option<(FxIndexSet<QueryEdge>, FxHashSet<DatabaseKeyIndex>)>> = const { std::cell::Cell::new(None) };
}

/// Flattens the dependencies of `head` so that `head`'s origin only depends on finalized queries,
/// or salsa structs (input, tracked, interned).
fn flatten_cycle_dependencies(zalsa: &Zalsa, head: &mut QueryRevisions) {
    let (mut flattened, mut seen) = FLATTEN_MAPS.take().unwrap_or_default();

    debug_assert!(flattened.is_empty());
    debug_assert!(seen.is_empty());

    #[cfg(feature = "accumulator")]
    {
        assert!(
            head.accumulated_inputs.load().is_empty(),
            "Fixpoint iteration doesn't support accumulated values because it doesn't preserve the original query dependency tree."
        )
    }

    // Don't insert the key of `head` here. This is important to ensure that we copy over the
    // dependencies from this memo in the previous iteration.
    // e.g. if we have `a2 -> b2 -> a1`, we need to copy over `a`'s dependencies from iteration 1.
    let edges = head.origin.as_ref().edges();
    flattened.reserve(edges.len());

    for edge in head.origin.as_ref().edges() {
        match edge.kind() {
            QueryEdgeKind::Input(input) => {
                let ingredient = zalsa.lookup_ingredient(input.ingredient_index());
                ingredient.flatten_cycle_head_dependencies(
                    zalsa,
                    input.key_index(),
                    &mut flattened,
                    &mut seen,
                );
            }

            QueryEdgeKind::Output(_) => {
                // Unlike `ingredient.collect_flattened_cycle_inputs`, carry over outputs
                // created by the query head because those are owned by this query.
                flattened.insert(*edge);
            }
        }
    }

    head.origin
        .set_edges(flattened.drain(..).collect())
        .expect("Executing query to always be derived or derived untracked.");

    seen.clear();

    FLATTEN_MAPS.set(Some((flattened, seen)));
}
