# Defining the database struct

Now that we have defined a [jar](./jar.md), we need to create the **database struct**.
The database struct is where all the jars come together.
Typically it is only used by the "driver" of your application;
the one which starts up the program, supplies the inputs, and relays the outputs.

In `calc`, the database struct is in the [`db`] module, and it looks like this:

[`db`]: https://github.com/salsa-rs/salsa/blob/master/calc-example/calc/src/db.rs

```rust
{{#include ../../../calc-example/calc/src/db.rs:db_struct}}
```

The `#[salsa::db(...)]` attribute takes a list of all the jars to include.
It autogenerates various bits of glue, including the `salsa::HasJar<J>` impl for each jar `J` that the [`crate::Db` trait declared as a supertrait](./jar.md#defining-the-database-trait).

**The struct must have a field named `storage` whose type is `salsa::Storage<Self>`.**
The `storage` struct owns all the data for the jars listed in the `db` attribute.

In addition to `storage`, your type may have whatever other fields you need.
in this example, we added a `logs` field to store the log we use for testing.

Note that we derive the `Default` trait -- this is not required, but it's often a convenient way to let users instantiate your database.

## Implementing the `salsa::Database` trait

In addition to the struct itself, we must add an impl of `salsa::Database`:

```rust
{{#include ../../../calc-example/calc/src/db.rs:db_impl}}
```

The `salsa::Database` trait includes a method `salsa_event` that you can choose to override
to give yourself more insight into how salsa is executing.
`salsa_event` is invoked when notable events occur, such as a function being executed
or a result being re-used.
Its default behavior is just to log the event using the `log` facade, so if you do not override
`salsa_event`, and you setup the [`env_logger`](https://crates.io/crates/env_logger) crate,
you can run your program with `RUST_LOG=salsa` to view what is happening.

In our case, we are going to override the method to both issue a debug event (viewable with `RUST_LOG=calc`)
and push some logging events for later observation.

## Implementing the `salsa::ParallelDatabase` trait

If you want to permit accessing your database from multiple threads at once, then you also need to implement the `ParallelDatabase` trait:

```rust
{{#include ../../../calc-example/calc/src/db.rs:par_db_impl}}
```

The `ParallelDatabase` impl needs to supply some sort of value for every field in your database.
The `storage` field provides a `snapshot` method for this purpose, but you have to figure out the best solution for custom fields.
In this example, we can simply clone the `logs` field as well, since it's an `Arc` that is meant to be shared across threads.

## Implementing the `PushLog` trait

If you recall, the `crate::Db` trait that [we defined earlier](./jar.md#defining-the-database-trait)
had the `PushLog` trait as a supertrait.
Because this is not a sala trait, it's our job to generate an impl for it.

```rust
{{#include ../../../calc-example/calc/src/db.rs:PushLogImpl}}
```

We also add some additional method to the `Database` that can only be used by tests:

```rust
{{#include ../../../calc-example/calc/src/db.rs:LoggingSupportCode}}
```

## Implementing the traits for each jar

The `Database` struct also needs to implement the [database traits for each jar](./jar.md#database-trait-for-the-jar).
In our case, though, we already wrote that impl as a [blanket impl alongside the jar itself](./jar.md#implementing-the-database-trait-for-the-jar),
so no action is needed.
This is the recommended strategy unless your trait has custom members that depend on fields of the `Database` itself
(for example, sometimes the `Database` holds some kind of custom resource that you want to give access to).
