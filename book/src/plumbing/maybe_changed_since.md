# Maybe changed since

```rust,no_run,noplayground
{{#include ../../../src/plumbing.rs:maybe_changed_since}}
```

The `maybe_changed_since` operation computes whether a query's value *may have changed* since the given revision.

## Input queries

Input queries are set explicitly by the user. `maybe_changed_since` can therefore just check when the value was last set and compare.

## Interned queries

## Derived queries


