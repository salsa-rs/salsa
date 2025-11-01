# Cycle handling

By default, when Salsa detects a cycle in the computation graph, Salsa will panic with a message naming the "cycle head"; this is the query that was called while it was also on the active query stack, creating a cycle.

Salsa also supports recovering from query cycles via fixed-point iteration. Fixed-point iteration is only usable if the queries which may be involved in a cycle are monotone and operate on a value domain which is a partial order with fixed height. Effectively, this means that the queries' output must always be "larger" than its input, and there must be some "maximum" or "top" value. This ensures that fixed-point iteration will converge to a value. (A typical case would be queries operating on types, which form a partial order with a "top" type.)

In order to support fixed-point iteration for a query, provide the `cycle_fn` and `cycle_initial` arguments to `salsa::tracked`:

```rust
#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query(db: &dyn salsa::Database) -> u32 {
    // ...
}

fn cycle_fn(_db: &dyn KnobsDatabase, _id: salsa::Id, _last_provisional_value: &u32, value: u32, _count: u32) -> u32 {
    value
}

fn cycle_initial(_db: &dyn KnobsDatabase, _id: salsa::Id) -> u32 {
    0
}
```

The `cycle_fn` is optional. The default implementation always returns the computed `value`.

If `query` becomes the head of a cycle (that is, `query` is executing and on the active query stack, it calls `query2`, `query2` calls `query3`, and `query3` calls `query` again -- there could be any number of queries involved in the cycle), the `cycle_initial` will be called to generate an "initial" value for `query` in the fixed-point computation. (The initial value should usually be the "bottom" value in the partial order.) All queries in the cycle will compute a provisional result based on this initial value for the cycle head. That is, `query3` will compute a provisional result using the initial value for `query`, `query2` will compute a provisional result using this provisional value for `query3`. When `cycle2` returns its provisional result back to `cycle`, `cycle` will observe that it has received a provisional result from its own cycle, and will call the `cycle_fn` (with the last provisional value, the newly computed value, and the number of iterations that have occurred so far). The `cycle_fn` can return the `value` parameter to continue iterating with the computed value, or return a different value (a fallback value) to continue iteration with that value instead.

The cycle will iterate until it converges: that is, until the value returned by `cycle_fn` equals the value from the previous iteration.

If a cycle iterates more than 200 times, Salsa will panic rather than iterate forever.

## All potential cycle heads must set `cycle_fn` and `cycle_initial`

Consider a two-query cycle where `query_a` calls `query_b`, and `query_b` calls `query_a`. If `query_a` is called first, then it will become the "cycle head", but if `query_b` is called first, then `query_b` will be the cycle head. In order for a cycle to use fixed-point iteration instead of panicking, the cycle head must set `cycle_fn` and `cycle_initial`. This means that in order to be robust against varying query execution order, both `query_a` and `query_b` must set `cycle_fn` and `cycle_initial`.

## Ensuring convergence

Fixed-point iteration is a powerful tool, but is also easy to misuse, potentially resulting in infinite iteration. To avoid this, ensure that all queries participating in fixpoint iteration are deterministic and monotone.

To guarantee convergence, you can leverage the `last_provisional_value` (3rd parameter) received by `cycle_fn`.
When the `cycle_fn` receives a newly computed value, you can implement a strategy that references the last provisional value to "join" values or "widen" it and return a fallback value. This ensures monotonicity of the calculation and suppresses infinite oscillation of values between cycles. For example:

Also, in fixed-point iteration, it is advantageous to be able to identify which cycle head seeded a value. By embedding a `salsa::Id` (2nd parameter) in the initial value as a "cycle marker", the recovery function can detect self-originated recursion.

## Calling Salsa queries from within `cycle_fn` or `cycle_initial`

It is permitted to call other Salsa queries from within the `cycle_fn` and `cycle_initial` functions. However, if these functions re-enter the same cycle, this can lead to unpredictable results. Take care which queries are called from within cycle-recovery functions, and avoid triggering further cycles.
