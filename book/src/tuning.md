# Tuning Salsa

## Cache eviction

By default, tracked functions retain every memoized value for as long as its key
remains in the database. Configure an eviction policy to bound the number of
values retained by a function:

```rust
#[salsa::tracked(eviction(policy = sieve, capacity = 128))]
fn parse(db: &dyn Db, input: SourceFile) -> Ast {
    // ...
}
```

The `capacity` option is required. The optional `policy` defaults to `sieve`.
Salsa supports two policies:

- `sieve` is recommended. Its lock-free cache-hit path avoids serializing
  concurrent accesses, and it often achieves better cache efficiency (a lower
  miss ratio) than LRU.
- `lru` maintains exact least-recently-used order, but takes an exclusive lock on
  every access.

See [eviction policies] for their behavior and tradeoffs.

Eviction drops memoized values while retaining query keys and dependency
metadata. A later access can therefore recompute an evicted value, while queries
that depend on it can still determine whether their own values may have changed.

[eviction policies]: ./plumbing/terminology/eviction.md

### Zero-Cost When Disabled

When `eviction` is absent, Salsa uses a no-op policy that is optimized away by
the compiler. Functions without cache eviction therefore have no policy
bookkeeping on cache hits.

### Runtime Capacity Adjustment

For functions with eviction configured, you can adjust the capacity at runtime:

```rust
#[salsa::tracked(eviction(capacity = 128))]
fn my_query(db: &dyn Db, input: MyInput) -> Output {
    // ...
}

// Later, adjust the capacity:
my_query::set_eviction_capacity(&mut db, 256);
```

The `set_eviction_capacity` method is only generated for functions with an
`eviction` option. Setting the capacity to zero disables eviction by that policy.

### Memory Management

Eviction removes memoized values, not query keys or dependency metadata. Salsa
also reclaims stale tracked outputs and unused low-durability interned values.
Input identities remain until the database is dropped.

## Intern Queries

Intern queries can make key lookup cheaper, save memory, and
avoid the need for [`Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html).

Interning is especially useful for queries that involve nested,
tree-like data structures.

See:

- The [`calc` example](https://github.com/salsa-rs/salsa/tree/master/examples/calc),
  which uses interning.

## Cancellation

Queries that are no longer needed due to concurrent writes or changes in dependencies are cancelled
by Salsa. Each access of an intermediate query is a potential cancellation point. Cancellation is
implemented via panicking, and Salsa internals are intended to be panic-safe.

If you have a query that contains a long loop which does not execute any intermediate queries,
salsa won't be able to cancel it automatically. You may wish to check for cancellation yourself
by invoking `db.unwind_if_revision_cancelled()`.

For more details on cancellation, see the tests for cancellation behavior in the Salsa repo.
