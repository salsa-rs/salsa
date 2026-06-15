# Tuning Salsa

## Cache Eviction

By default, memoized values are never evicted. You can enable adaptive,
generational eviction with the existing `lru` option:

```rust
#[salsa::tracked(lru = 128)]
fn parse(db: &dyn Db, input: SourceFile) -> Ast {
    // ...
}
```

The value is currently a minimum collection-growth threshold, not a hard
capacity. New values receive multiple collection epochs of grace. Values that
are reused across epochs move into older generations, while values that remain
cold across repeated inspections are evicted. Collection epochs advance only
after sufficient resident growth and only inspect the due generation.

### Zero-Cost When Disabled

When no `lru` capacity is specified (the default), Salsa uses a no-op eviction
policy that is completely optimized away by the compiler. This means there is
zero runtime overhead for functions that don't need cache eviction.

### Runtime Threshold Adjustment

For functions with eviction enabled, the existing method adjusts the minimum
growth threshold at runtime:

```rust
#[salsa::tracked(lru = 128)]
fn my_query(db: &dyn Db, input: MyInput) -> Output {
    // ...
}

// Later, adjust the collection threshold:
my_query::set_lru_capacity(db, 256);
```

The method retains its existing name while the eviction API is being evaluated.
It is only generated for functions that have an `lru` attribute.

### Memory Management

Eviction drops memoized values but retains their dependency information. The
collector does not enforce an absolute memory bound: its goal is to prevent
unused values from accumulating indefinitely while avoiding synchronization on
query fetches.

## Intern Queries

Intern queries can make key lookup cheaper, save memory, and
avoid the need for [`Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html).

Interning is especially useful for queries that involve nested,
tree-like data structures.

See:

- The [`compiler` example](https://github.com/salsa-rs/salsa/blob/master/examples/compiler/main.rs),
  which uses interning.

## Cancellation

Queries that are no longer needed due to concurrent writes or changes in dependencies are cancelled
by Salsa. Each access of an intermediate query is a potential cancellation point. Cancellation is
implemented via panicking, and Salsa internals are intended to be panic-safe.

If you have a query that contains a long loop which does not execute any intermediate queries,
salsa won't be able to cancel it automatically. You may wish to check for cancellation yourself
by invoking `db.unwind_if_cancelled()`.

For more details on cancellation, see the tests for cancellation behavior in the Salsa repo.
