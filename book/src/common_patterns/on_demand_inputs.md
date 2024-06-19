# On-Demand (Lazy) Inputs

Salsa inputs work best if you can easily provide all of the inputs upfront.
However sometimes the set of inputs is not known beforehand.

A typical example is reading files from disk.
While it is possible to eagerly scan a particular directory and create an in-memory file tree as salsa input structs, a more straight-forward approach is to read the files lazily.
That is, when a query requests the text of a file for the first time:

1. Read the file from disk and cache it.
2. Setup a file-system watcher for this path.
3. Update the cached file when the watcher sends a change notification.

This is possible to achieve in salsa, by caching the inputs in your database structs and adding a method to the database trait to retrieve them out of this cache.

A complete, runnable file-watching example can be found in [the lazy-input example](https://github.com/salsa-rs/salsa/tree/master/examples/lazy-input).

The setup looks roughly like this:

```rust,ignore
{{#include ../../../examples/lazy-input/main.rs:db}}
```

- We declare a method on the `Db` trait that gives us a `File` input on-demand (it only requires a `&dyn Db` not a `&mut dyn Db`).
- There should only be one input struct per file, so we implement that method using a cache (`DashMap` is like a `RwLock<HashMap>`).

The driving code that's doing the top-level queries is then in charge of updating the file contents when a file-change notification arrives.
It does this by updating the Salsa input in the same way that you would update any other input.

Here we implement a simple driving loop, that recompiles the code whenever a file changes.
You can use the logs to check that only the queries that could have changed are re-evaluated.

```rust,ignore
{{#include ../../../examples/lazy-input/main.rs:main}}
```
