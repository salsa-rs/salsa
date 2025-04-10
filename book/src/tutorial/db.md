# Defining the database struct

First, we need to create the **database struct**.
Typically it is only used by the "driver" of your application;
the one which starts up the program, supplies the inputs, and relays the outputs.

In `calc`, the database struct is in the [`db`] module, and it looks like this:

[`db`]: https://github.com/salsa-rs/salsa/blob/master/examples/calc/db.rs

```rust
{{#include ../../../examples/calc/db.rs:db_struct}}
```

The `#[salsa::db]` attribute marks the struct as a database.
It must have a field named `storage` whose type is `salsa::Storage<Self>`, but it can also contain whatever other fields you want.

## Implementing the `salsa::Database` trait

In addition to the struct itself, we must add an impl of `salsa::Database`:

```rust
{{#include ../../../examples/calc/db.rs:db_impl}}
```

## Implementing the `salsa::ParallelDatabase` trait

If you want to permit accessing your database from multiple threads at once, then you also need to implement the `ParallelDatabase` trait:

```rust
{{#include ../../../examples/calc/db.rs:par_db_impl}}
```
