# Tuning Salsa

## Cache Eviction (LRU)

Salsa supports Least Recently Used (LRU) cache eviction for tracked functions.
By default, memoized values are never evicted (unbounded cache). You can enable
LRU eviction by specifying a capacity at compile time:

```rust
#[salsa::tracked(lru = 128)]
fn parse(db: &dyn Db, input: SourceFile) -> Ast {
    // ...
}
```

With `lru = 128`, Salsa will keep at most 128 memoized values for this function.
When the cache exceeds this capacity, the least recently used values are evicted
at the start of each new revision.

### Zero-Cost When Disabled

When no `lru` capacity is specified (the default), Salsa uses a no-op eviction
policy that is completely optimized away by the compiler. This means there is
zero runtime overhead for functions that don't need cache eviction.

### Runtime Capacity Adjustment

For functions with LRU enabled, you can adjust the capacity at runtime:

```rust
#[salsa::tracked(lru = 128)]
fn my_query(db: &dyn Db, input: MyInput) -> Output {
    // ...
}

// Later, adjust the capacity:
my_query::set_lru_capacity(&mut db, 256);
```

**Note:** The `set_lru_capacity` method is only generated for functions that have
an `lru` attribute. Functions without LRU enabled do not have this method.

### Within-Revision Collection

For values that are cheap to recompute and useful only during a short burst of
queries, use the `volatile` option:

```rust
#[salsa::tracked(volatile = 4096)]
fn lower(db: &dyn Db, input: SourceFile) -> Arc<LoweredFile> {
    // ...
}
```

Unlike `lru`, volatile collection runs within a revision. Once the cache reaches
its configured capacity, each new value replaces a randomly selected resident
value. Cache hits require no eviction bookkeeping or synchronization.

Volatile queries must return values using `returns(copy)` or `returns(clone)`
(the default). Reference return modes such as `returns(ref)` are rejected
because volatile values may be dropped before the next revision.

### Memory Management

LRU evicts memoized values, not query keys or dependency metadata. Salsa also
reclaims stale tracked outputs and unused low-durability interned values. Input
identities remain until the database is dropped.

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
