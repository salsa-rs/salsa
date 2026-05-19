#![cfg(feature = "inventory")]

//! Reproduction for the `src/function/execute.rs` early-break path (the
//! `if cycle_heads.is_empty()` arm of the `execute_maybe_iterate` fixpoint
//! loop, around the `break (new_value, completed_query)` after the
//! `iteration_count.is_initial()` check).
//!
//! `cycle.rs` documents the convergence contract: a cycle is final only once
//! "the value returned by `cycle_fn` is the same as the provisional value
//! from the previous iteration". Every other exit from the fixpoint loop
//! enforces this via `C::values_equal(&new_value, last_provisional_value)`.
//! The empty-cycle-heads arm does not: when a query that *was* mid-fixpoint
//! (`!iteration_count.is_initial()`) computes a value whose cycle-head set is
//! now empty (because it conditionally dropped its recursive dependency on a
//! late iteration, the case `cycle.rs` flags via `CycleHead::removed`), the
//! loop breaks and memoizes that freshly computed value with no convergence
//! comparison and without calling the user's `cycle_fn` for that iterate.
//!
//! Construction. `query_x` is a self-cycle head with a monotone, bounded
//! recurrence `inner + 1` capped at `cap`, whose unique fixpoint is `cap`.
//! While its recursive self-dependency is live it iterates normally (its
//! `cycle_fn` is consulted each iteration). On a chosen iteration *before*
//! the recurrence has reached `cap` it stops recursing and returns an
//! unrelated value (`V(99)`), dropping its self-dependency so its cycle-head
//! set becomes empty. On HEAD that iteration exits via the early-break and
//! `V(99)` is memoized as the final result, even though the recurrence has a
//! well-defined fixpoint `V(cap)` it had not yet reached, the last
//! provisional value was far from `V(99)`, and the user's `cycle_fn` was
//! never asked about `V(99)`.

use std::cell::Cell;

use salsa::Database as Db;

mod common;

#[derive(Debug, PartialEq, Eq, Clone, Copy, salsa::Update)]
struct V(u32);

#[salsa::input]
struct Input {
    /// Upper bound of the monotone recurrence; the unique fixpoint is `V(cap)`.
    cap: u32,
    /// 1-based entry count of `query_x` on which it drops the recursive
    /// self-dependency. Chosen below the iterations needed to reach `cap`, so
    /// the drop happens while the recurrence is provably not yet converged.
    drop_at: u32,
}

thread_local! {
    static X_RUNS: Cell<u32> = const { Cell::new(0) };
}

fn x_initial(_db: &dyn Db, _id: salsa::Id, _input: Input) -> V {
    V(0)
}

fn x_recover(_db: &dyn Db, _c: &salsa::Cycle, _last: &V, value: V, _i: Input) -> V {
    // Drive `query_x` toward its own fixpoint normally (identity recovery).
    value
}

#[salsa::tracked(cycle_fn = x_recover, cycle_initial = x_initial)]
fn query_x(db: &dyn Db, input: Input) -> V {
    let run = X_RUNS.with(|c| {
        let n = c.get() + 1;
        c.set(n);
        n
    });

    if run == input.drop_at(db) {
        // Conditional dependency dropped: no recursive self-call this
        // iteration, so `query_x`'s cycle-head set is empty here. `V(99)` is
        // deliberately not on the recurrence trajectory.
        return V(99);
    }

    // Recursive self-dependency live: keeps `query_x` a cycle head and keeps
    // the fixpoint iterating. Monotone, bounded recurrence; fixpoint = V(cap).
    let inner = query_x(db, input);
    V((inner.0 + 1).min(input.cap(db)))
}

#[test]
fn drop_iteration_must_not_memoize_unconverged_value() {
    let db = salsa::DatabaseImpl::new();
    // cap = 10 needs ~10 iterations to converge; drop on the 3rd entry, well
    // before the recurrence reaches its fixpoint.
    let input = Input::new(&db, 10, 3);

    let result = query_x(&db, input);

    // The recurrence `inner+1` capped at 10 has the unique fixpoint V(10).
    // The drop happened at iterate V(2)-ish, far from V(10), and returned an
    // off-trajectory V(99). A sound termination must reach the real fixpoint
    // V(10) (or hit the MAX_ITERATIONS backstop for a genuinely non-
    // converging recurrence, which this is not). It must NOT silently
    // memoize the off-trajectory drop value V(99).
    assert_eq!(
        result,
        V(10),
        "BUG: salsa memoized the off-trajectory drop value V({}) as the \
         final result of a recurrence whose unique fixpoint is V(10). The \
         empty-cycle-heads early-break in execute.rs skipped the convergence \
         check, so the cycle was finalized at a value it never converged to \
         and that the user's cycle_fn was never consulted about.",
        result.0
    );
}
