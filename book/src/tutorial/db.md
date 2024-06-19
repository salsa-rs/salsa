# Defining the database struct

Now that we have defined a [jar](./jar.md), we need to create the **database struct**.
The database struct is where all the jars come together.
Typically it is only used by the "driver" of your application;
the one which starts up the program, supplies the inputs, and relays the outputs.

In `calc`, the database struct is in the [`db`] module, and it looks like this:

[`db`]: https://github.com/salsa-rs/salsa/blob/master/examples/calc/db.rs

```rust
{{#include ../../../examples/calc/db.rs:db_struct}}
```

The `#[salsa::db(...)]` attribute takes a list of all the jars to include.
The struct must have a field named `storage` whose type is `salsa::Storage<Self>`, but it can also contain whatever other fields you want.
The `storage` struct owns all the data for the jars listed in the `db` attribute.

The `salsa::db` attribute autogenerates a bunch of impls for things like the `salsa::HasJar<crate::Jar>` trait that we saw earlier.

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

## Implementing the traits for each jar

The `Database` struct also needs to implement the [database traits for each jar](./jar.md#database-trait-for-the-jar).
In our case, though, we already wrote that impl as a [blanket impl alongside the jar itself](./jar.md#implementing-the-database-trait-for-the-jar),
so no action is needed.
This is the recommended strategy unless your trait has custom members that depend on fields of the `Database` itself
(for example, sometimes the `Database` holds some kind of custom resource that you want to give access to).
