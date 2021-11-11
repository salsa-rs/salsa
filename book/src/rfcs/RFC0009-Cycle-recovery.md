# Description/title

## Metadata

* Author: nikomatsakis
* Date: 2021-10-31
* Introduced in: https://github.com/salsa-rs/salsa/pull/285

## Summary

* Permit cycle recovery as long as at least one participant has recovery enabled.
* Modify cycle recovery to take a `&Cycle`.
* Introduce `Cycle` type that carries information about a cycle and lists participants in a deterministic order.

[RFC 7]: ./RFC0007-Opinionated-Cancelation.md

## Motivation

Cycle recovery has been found to have some subtle bugs that could lead to panics. Furthermore, the existing cycle recovery APIs require all participants in a cycle to have recovery enabled and give limited and non-deterministic information. This RFC tweaks the user exposed APIs to correct these shortcomings. It also describes a major overhaul of how cycles are handled internally.

## User's guide

By default, cycles in the computation graph are considered a "programmer bug" and result in a panic. Sometimes, though, cycles are outside of the programmer's control. Salsa provides mechanisms to recover from cycles that can help in those cases.

### Default cycle handling: panic

By default, when Salsa detects a cycle in the computation graph, Salsa will panic with a `salsa::Cycle` as the panic value. Your queries should not attempt to catch this value; rather, the `salsa::Cycle` is meant to be caught by the outermost thread, which can print out information from it to diagnose what went wrong. The `Cycle` type offers a few methods for inspecting the participants in the cycle:

* `participant_keys` -- returns an iterator over the `DatabaseKeyIndex` for each participant in the cycle.
* `all_participants` -- returns an iterator over `String` values for each participant in the cycle (debug output).
* `unexpected_participants` -- returns an iterator over `String` values for each participant in the cycle that doesn't have recovery information (see next section).

`Cycle` implements `Debug`, but because the standard trait doesn't provide access to the database, the output can be kind of inscrutable. To get more readable `Debug` values, use the method `cycle.debug(db)`, which returns an `impl Debug` that is more readable.

### Cycle recovery

Panicking when a cycle occurs is ok for situations where you believe a cycle is impossible. But sometimes cycles can result from illegal user input and cannot be statically prevented. In these cases, you might prefer to gracefully recover from a cycle rather than panicking the entire query. Salsa supports that with the idea of *cycle recovery*.

To use cycle recovery, you annotate potential participants in the cycle with a `#[salsa::recover(my_recover_fn)]` attribute. When a cycle occurs, if any participant P has recovery information, then no panic occurs. Instead, the execution of P is aborted and P will execute the recovery function to generate its result. Participants in the cycle that do not have recovery information continue executing as normal, using this recovery result.

The recovery function has a similar signature to a query function. It is given a reference to your database along with a `salsa::Cycle` describing the cycle that occurred; it returns the result of the query. Example:

```rust
fn my_recover_fn(
    db: &dyn MyDatabase,
    cycle: &salsa::Cycle,
) -> MyResultValue
```

The `db` and `cycle` argument can be used to prepare a useful error message for your users. 

**Important:** Although the recovery function is given a `db` handle, you should be careful to avoid creating a cycle from within recovery or invoking queries that may be participating in the current cycle. Attempting to do so can result in inconsistent results.

### Figuring out why recovery did not work

If a cycle occurs and *some* of the participant queries have `#[salsa::recover]` annotations and others do not, then the query will be treated as irrecoverable and will simply panic. You can use the `Cycle::unexpected_participants` method to figure out why recovery did not succeed and add the appropriate `#[salsa::recover]` annotations.

## Reference guide

This RFC accompanies a rather long and complex PR with a number of changes to the implementation. We summarize the most important points here.

### Cross-thread blocking

The interface for blocking across threads now works as follows:

* When one thread `T1` wishes to block on a query `Q` being executed by another thread `T2`, it invokes `Runtime::try_block_on`. This will check for cycles. Assuming no cycle is detected, it will block `T1` until `T2` has completed with `Q`. At that point, `T1` reawakens. However, we don't know the result of executing `Q`, so `T1` now has to "retry". Typically, this will result in successfully reading the cached value.
* While `T1` is blocking, the runtime moves its query stack (a `Vec`) into the shared dependency graph data structure. When `T1` reawakens, it recovers ownership of its query stack before returning from `try_block_on`.

### Cycle detection

When a thread `T1` attempts to execute a query `Q`, it will try to load the value for `Q` from the memoization tables. If it finds an `InProgress` marker, that indicates that `Q` is currently being computed. This indicates a potential cycle. `T1` will then try to block on the query `Q`:

* If `Q` is also being computed by `T1`, then there is a cycle.
* Otherwise, if `Q` is being computed by some other thread `T2`, we have to check whether `T2` is (transitively) blocked on `T1`. If so, there is a cycle.

These two cases are handled internally by the `Runtime::try_block_on` function. Detecting the intra-thread cycle case is easy; to detect cross-thread cycles, the runtime maintains a dependency DAG between threads (identified by `RuntimeId`). Before adding an edge `T1 -> T2` (i.e., `T1` is blocked waiting for `T2`) into the DAG, it checks whether a path exists from `T2` to `T1`. If so, we have a cycle and the edge cannot be added (then the DAG would not longer be acyclic).

When a cycle is detected, the current thread `T1` has full access to the query stacks that are participating in the cycle. Consider: naturally, `T1` has access to its own stack. There is also a path `T2 -> ... -> Tn -> T1` of blocked threads. Each of the blocked threads `T2 ..= Tn` will have moved their query stacks into the dependency graph, so those query stacks are available for inspection.

Using the available stacks, we can create a list of cycle participants `Q0 ... Qn`. We can then check the cycle recovery setting for `Q0 ... Qn`. If any queries have the "panic" setting, then the cycle is irrecoverable, and we can throw a `Cancelled` error. This will result in the various queries being unrolled and their memoized values being removed from the tables. If all the queries have the "recover" setting, then we can commence with recovery.

### Cycle recovery

Cycle recovery begins with a set of active cycle participants `Q0 ... Qn`, all of which are tagged with a recovery function. For those querries, we compute a maximal `changed_at` and a minimum `duration` for all participating queries. These values reflect all the inputs (external to the query) which were accessed thus far. Unless those inputs change, we can be assured that any attempt to re-execute `Q0 ... Qn` will result in the same cycle.

Next, we modify the query stack frame for each participant `Q0 .. Qn` to reflect that it is recovering from a cycle:

* We upgrade its `changed_at` and `durability` to reflect the values for the cycle as a whole.
* We store the cycle participants in the `cycle` field.

We will now begin unwinding the stack and computing the recovery values for `Q0 .. Qn` in turn. Query recovery is an "internal" affair: that is, when a query `Qi` stores its recovery value, it returns a normal-looking value to its caller (though the changed-at/durability values will reflect the cycle as a whole). If the caller was not a participant in the cycle, it can simply use that value like it normally would and continue execution.

Queries that *are* participants in the cycle, however, are marked by their `cycle` field. Consider some query `Qi` in the middle of the cycle. `Qi` will receive the return value from the next query `Qi+1` in the cycle as normal. `Qi` will then record its dependency on `Qi+1` and, in the process, observe that the cycle field in `Qi`'s stack frame is set. Thus `Qi` can judge that it was a participant in some cycle and it will panic to avoid continuing to execute. This panic is caught by the code that invoked `Qi`'s query function, and we then invoke the recovery function instead to produce the recovery value for `Qi`. This is stored into the memoization tables like any other value, and `Qi` returns to `Qi-1`, and the process continues. After `Q0` returns, though, its caller was not a participant in the cycle, and thus doesn't have the cycle flag set. That caller can just continue as normal.

There is one other edge case to consider. The query `Qn` was in the process of invoking `Q0` when the cycle was uncovered. That process needs to conclude with *some* value so that `Qn` can observe the cycle field and initiative recovery. For that, we simply invoke the recovery function for `Q0`. Thus, the `Q0` recovery function will in fact be invoked twice. Once to produce a value for `Qn` (which will probably be ignored...) and once to produce the final memoized value for `Q0`.

## Frequently asked questions

### Why have other threads retry instead of giving them the value?

In the past, when one thread T1 blocked on some query Q being executed by another thread T2, we would create a custom channel between the threads. T2 would then send the result of Q directly to T1, and T1 had no need to retry. This mechanism was simplified in this RFC because we don't always have a value available: sometimes the cycle results when T2 is just verifying whether a memoized value is still valid. In that case, the value may not have been computed, and so when T1 retries it will in fact go on to compute the value. (Previously, this case was overlooked by the cycle handling logic and resulted in a panic.)

### Why do we invoke the recovery fn for Q0 twice? Why not have queries return a `Result`?

In the section on cycle recovery, we describe how the query `Qn` needs to get *some* value from `Q0` before it can initiative recovery. Currently, we handle this by invoking the `Q0` recovery function twice. However, `Qn` only needs this value because the function signature for `probe` needs to return *something*; it never actually reads this value, since it begins cycle recovery before it could do so. We could handle this by having the function signature return a `Result` or an `Option` instead. In the case of a cycle, we would return `Err` or `None`. We didn't do this because cycle recovery is meant to be an exceptional case and is not required to be particularly fast. It seemed better to optimize for the 'common case' of no cycle.

### Why not invoke the recovery functions all at once?

The code currently unwinds frame by frame and invokes recovery as it goes. Another option might be to invoke the recovery function for all participants in the cycle up-front. This would be fine, but it's a bit difficult to do, since the types for each cycle are different, and the `Runtime` code doesn't know what they are. We also don't have access to the memoization tables and so forth.