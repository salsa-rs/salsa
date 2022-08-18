# Database and runtime

A salsa database struct is declared by the user with the `#[salsa::db]` annotation.
It contains all the data that the program needs to execute:

```rust,ignore
#[salsa::db(jar0...jarn)]
struct MyDatabase {
    storage: Storage<Self>,
    maybe_other_fields: u32,
}
```

This data is divided into two categories:

* Salsa-governed storage, contained in the `Storage<Self>` field. This data is mandatory.
* Other fields (like `maybe_other_fields`) defined by the user. This can be anything. This allows for you to give access to special resources or whatever.

## Parallel handles

When used across parallel threads, the database type defined by the user must support a "snapshot" operation.
This snapshot should create a clone of the database that can be used by the parallel threads.
The `Storage` operation itself supports `snapshot`.
The `Snapshot` method returns a `Snapshot<DB>` type, which prevents these clones from being accessed via an `&mut` reference.

## The Storage struct

The salsa `Storage` struct contains all the data that salsa itself will use and work with.
There are three key bits of data:

* The `Shared` struct, which contains the data stored across all snapshots. This is primarily the ingredients described in the [jars and ingredients chapter](./jars_and_ingredients.md), but it also contains some synchronization information (a cond var). This is used for cancellation, as described below.
    * The data in the `Shared` struct is only shared across threads when other threads are active. Some operations, like mutating an input, require an `&mut` handle to the `Shared` struct. This is obtained by using the `Arc::get_mut` methods; obviously this is only possible when all snapshots and threads have ceased executing, since there must be a single handle to the `Arc`.
* The `Routes` struct, which contains the information to find any particular ingredient -- this is also shared across all handles, and its construction is also described in the [jars and ingredients chapter](./jars_and_ingredients.md). The routes are separated out from the `Shared` struct because they are truly immutable at all times, and we want to be able to hold a handle to them while getting `&mut` access to the `Shared` struct.
* The `Runtime` struct, which is specific to a particular database instance. It contains the data for a single active thread, along with some links to shraed data of its own.

## The Salsa runtime

The salsa runtime offers helper methods that are accessed by the ingredients.
It tracks, for example, the active query stack, and contains methods for adding dependencies between queries (e.g., `report_tracked_read`) or [resolving cycles](./cycles.md).
It also tracks the current revision and information about when values with low or high durability last changed.

Basically, the ingredient structures store the "data at rest" -- like memoized values -- and things that are "per ingredient".

The runtime stores the "active, in-progress" data, such as which queries are on the stack, and/or the dependencies accessed by the currently active query.