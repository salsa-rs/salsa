# Recovering via fallback

Panicking when a cycle occurs is ok for situations where you believe a cycle is impossible. But sometimes cycles can result from illegal user input and cannot be statically prevented. In these cases, you might prefer to gracefully recover from a cycle rather than panicking the entire query. Salsa supports that with the idea of *cycle recovery*.

To use cycle recovery, you annotate potential participants in the cycle with a `#[salsa::cycle(my_recover_fn)]` attribute. When a cycle occurs, if any participant P has recovery information, then no panic occurs. Instead, the execution of P is aborted and P will execute the recovery function to generate its result. Participants in the cycle that do not have recovery information continue executing as normal, using this recovery result.

The recovery function has a similar signature to a query function. It is given a reference to your database along with a `salsa::Cycle` describing the cycle that occurred; it returns the result of the query. Example:

```rust
fn my_recover_fn(
    db: &dyn MyDatabase,
    cycle: &salsa::Cycle,
) -> MyResultValue
```

The `db` and `cycle` argument can be used to prepare a useful error message for your users.

**Important:** Although the recovery function is given a `db` handle, you should be careful to avoid creating a cycle from within recovery or invoking queries that may be participating in the current cycle. Attempting to do so can result in inconsistent results.
