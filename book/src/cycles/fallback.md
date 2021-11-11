# Recovering via fallback

Panicking when a cycle occurs is ok for situations where you believe a cycle is impossible. But sometimes cycles can result from illegal user input and cannot be statically prevented. In these cases, you might prefer to gracefully recover from a cycle rather than panicking the entire query. Salsa supports that with the idea of *cycle recovery*.

To use cycle recovery, you annotate every potential participant in the cycle with a `#[salsa::recover(my_recover_fn)]` attribute. When a cycle occurs, if **all** participants have recovery information, then no panic will result. Instead, salsa will abort the execution of the cycle participants and invoke the recovery function `my_recover_fn` instead. The result of this recovery will be returned as the query result. 

The recovery function has a similar signature to a query function. It is given a reference to your database along with a `salsa::Cycle` describing the cycle that occurred; it returns the result of the query. Example:

```rust
fn my_recover_fn(
    db: &dyn MyDatabase,
    cycle: &salsa::Cycle,
) -> MyResultValue
```

The `db` and `cycle` argument can be used to prepare a useful error message for your users. 

**Important:** Although the recovery function is given a `db` handle, you should be careful to avoid creating a cycle from within recovery or invoking queries that may be participating in the current cycle. Attempting to do so can result in inconsistent results.

## Figuring out why recovery did not work

If a cycle occurs and *some* of the participant queries have `#[salsa::recover]` annotations and others do not, then the query will be treated as irrecoverable and will simply panic. You can use the `Cycle::unexpected_participants` method to figure out why recovery did not succeed and add the appropriate `#[salsa::recover]` annotations.