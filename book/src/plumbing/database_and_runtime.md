# Database and runtime

A salsa database struct is declared by the user with the `#[salsa::db]` annotation.
It contains all the data that the program needs to execute:

```rust,ignore
#[salsa::db]
struct MyDatabase {
    storage: Storage<Self>,
    maybe_other_fields: u32,
}
```

This data is divided into two categories:

- Salsa-governed storage, contained in the `Storage<Self>` field. This data is mandatory.
- Other fields (like `maybe_other_fields`) defined by the user. This can be anything. This allows for you to give access to special resources or whatever.

## Parallel handles

When used across parallel threads, the database type defined by the user must implement `Clone`.
Each clone can be used by the parallel threads.
The `Storage` type shares the ingredients, runtime, and memoized values between clones.
Each clone has its own active query stack.

## The Storage struct

The salsa `Storage` struct contains all the data that salsa itself will use and work with.
There are two key parts:

- The shared `Zalsa` data, which contains the ingredients, runtime, memoized values, and synchronization information. Some operations, like mutating an input, require an `&mut` handle to this data. This is obtained by using `Arc::get_mut`, which is only possible once all clones and parallel threads have ceased executing.
- The per-handle `ZalsaLocal` data, which is specific to a particular database instance. It contains the data for a single active thread, including the active query stack.

## Incrementing the revision counter

Salsa's general model is that there is a database and, potentially, multiple cloned handles.
Each clone owns another handle on the `Arc` in `Storage` that stores the ingredients.

Whenever the user attempts to do an `&mut` operation, such as modifying an input field, Salsa must
first cancel any parallel handles and wait for those threads to finish.
Once the other handles have completed, Salsa can use `Arc::get_mut` to get an `&mut` reference to the shared data.
This allows Salsa to get `&mut` access without unsafe code and
guarantees that it has successfully cancelled the other worker threads
(or gotten itself into a deadlock).

The key point is that Salsa cancels other workers before proceeding:

```rust
{{#include ../../../src/storage.rs:cancel_other_workers}}
```

## The Salsa runtime

The salsa runtime offers helper methods that are accessed by the ingredients.
It tracks, for example, the active query stack, and contains methods for adding dependencies between queries (e.g., `report_tracked_read`) or [resolving cycles](./cycles.md).
It also tracks the current revision and information about when values with low or high durability last changed.

Basically, the ingredient structures store the "data at rest" -- like memoized values -- and things that are "per ingredient".

The runtime stores the "active, in-progress" data, such as which queries are on the stack, and/or the dependencies accessed by the currently active query.
