# Query operations

Each of the query storage struct implements the `LocalQueryStorageOps` trait found in the [`plumbing`] module:

[`plumbing`]: https://github.com/salsa-rs/salsa/blob/master/src/plumbing.rs

```rust,no_run,noplayground
{{#include ../../../src/plumbing.rs:LocalQueryStorageOps}}
```

 which defines the basic operations that all queries support. The most important are these two:

* [Maybe changed since](./maybe_changed_since.md): Returns true if the value of the query (for the given key) may have changed since the given revision.
* [Fetch](./fetch.md): Returms the up-to-date value for the given K (or an error in the case of an "unrecovered" cycle).

## Local vs global storage

Each query has two kinds of storage:

* **Local storage:** Accessible only to the current thread. Used to store intermediate, thread-local values.
* **Global storage:** Accessible to all threads. Used for caching completed, memoized values.

The `LocalQueryStorageOps` trait is implemented on the local storage; it will access the global storage as needed.
