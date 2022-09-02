# Query operations

Each of the query storage struct implements the `QueryStorageOps` trait found in the [`plumbing`] module:

```rust,no_run,noplayground
{{#include ../../../src/plumbing.rs:QueryStorageOps}}
```

 which defines the basic operations that all queries support. The most important are these two:

* [maybe changed after](./maybe_changed_after.md): Returns true if the value of the query (for the given key) may have changed since the given revision.
* [Fetch](./fetch.md): Returns the up-to-date value for the given K (or an error in the case of an "unrecovered" cycle).

[`plumbing`]: https://github.com/salsa-rs/salsa/blob/master/src/plumbing.rs
