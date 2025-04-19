# Cycle handling

By default, when Salsa detects a cycle in the computation graph, Salsa will panic with a message naming the "cycle head"; this is the query that was called while it was also on the active query stack, creating a cycle.

Salsa supports three recovery modes: panicking (the default), fixpoint resolution and immediate fallback.

## Fixpoint Resolution

Fixed-point iteration is only usable if the queries which may be involved in a cycle are monotone and operate on a value domain which is a partial order with fixed height. Effectively, this means that the queries' output must always be "larger" than its input, and there must be some "maximum" or "top" value. This ensures that fixed-point iteration will converge to a value. (A typical case would be queries operating on types, which form a partial order with a "top" type.)

In order to support fixed-point iteration for a query, provide the `cycle_fn` and `cycle_initial` arguments to `salsa::tracked`:

```rust
#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial_fn)]
fn query(db: &dyn salsa::Database) -> u32 {
    // ...
}

fn cycle_fn(_db: &dyn KnobsDatabase, _value: &u32, _count: u32) -> salsa::CycleRecoveryAction<u32> {
    salsa::CycleRecoveryAction::Iterate
}

fn initial(_db: &dyn KnobsDatabase) -> u32 {
    0
}
```

If `query` becomes the head of a cycle (that is, `query` is executing and on the active query stack, it calls `query2`, `query2` calls `query3`, and `query3` calls `query` again -- there could be any number of queries involved in the cycle), the `initial_fn` will be called to generate an "initial" value for `query` in the fixed-point computation. (The initial value should usually be the "bottom" value in the partial order.) All queries in the cycle will compute a provisional result based on this initial value for the cycle head. That is, `query3` will compute a provisional result using the initial value for `query`, `query2` will compute a provisional result using this provisional value for `query3`. When `cycle2` returns its provisional result back to `cycle`, `cycle` will observe that it has received a provisional result from its own cycle, and will call the `cycle_fn` (with the current value and the number of iterations that have occurred so far). The `cycle_fn` can return `salsa::CycleRecoveryAction::Iterate` to indicate that the cycle should iterate again, or `salsa::CycleRecoveryAction::Fallback(value)` to indicate that the cycle should stop iterating and fall back to the value provided.

If the `cycle_fn` continues to return `Iterate`, the cycle will iterate until it converges: that is, until two successive iterations produce the same result.

If the `cycle_fn` returns `Fallback`, the cycle will iterate one last time and verify that the returned value is the same as the fallback value; that is, the fallback value results in a stable converged cycle. If not, Salsa will panic. It is not permitted to use a fallback value that does not converge, because this would leave the cycle in an unpredictable state, depending on the order of query execution.

### All potential cycle heads must set `cycle_fn` and `cycle_initial`

Consider a two-query cycle where `query_a` calls `query_b`, and `query_b` calls `query_a`. If `query_a` is called first, then it will become the "cycle head", but if `query_b` is called first, then `query_b` will be the cycle head. In order for a cycle to use fixed-point iteration instead of panicking, the cycle head must set `cycle_fn` and `cycle_initial`. This means that in order to be robust against varying query execution order, both `query_a` and `query_b` must set `cycle_fn` and `cycle_initial`.

### Ensuring convergence

Fixed-point iteration is a powerful tool, but is also easy to misuse, potentially resulting in infinite iteration. To avoid this, ensure that all queries participating in fixpoint iteration are deterministic and monotone.

### Calling Salsa queries from within `cycle_fn` or `cycle_initial`

It is permitted to call other Salsa queries from within the `cycle_fn` and `cycle_initial` functions. However, if these functions re-enter the same cycle, this can lead to unpredictable results. Take care which queries are called from within cycle-recovery functions, and avoid triggering further cycles.

## Immediate Fallback

This mode of cycle handling causes query calls that result in a cycle to immediately return with a fallback value.

In order to support this fallback for a query, provide the `cycle_result` argument to `salsa::tracked`:

```rust
#[salsa::tracked(cycle_result=fallback)]
fn query(db: &dyn salsa::Database) -> u32 {
    // ...
}

fn fallback(_db: &dyn KnobsDatabase) -> u32 {
    0
}
```

### Observable execution order

One problem with this fallback mode is that execution order / entry points become part of the query computation and can affect the results of queries containing cycles.
This introduces a potential form on non-determinism depending on the query graph when multiple differing cycling queries are involved.
Due to this, when an immediate fallback cycle occurs, salsa walks back the active query stacks to verify that the cycle does not occur within the context of another non-panic cycle query.
In other words, it is only valid to immediate fallback cycle recover for a query if either all ancestors queries are panic cycle queries or if the cycle is immediate self-referential.
