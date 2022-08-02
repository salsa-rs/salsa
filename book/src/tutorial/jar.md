# Jars and databases

Salsa programs are composed in **jars**[^jar].
A jar is just a fancy name for a struct whose fields contain the hashmaps and other state required to implement salsa concepts like [memoized function](../overview.md#memoized-functions) or [entity](../overview.md#entity-values)/[interned](../overview.md#interned-values) structs.
Typically you have one jar per crate, but that is not required.
When you declare the salsa database, you will give it a list of all the jar structs in your program, and it will allocate one of each so as to have all the storage it needs.

Each time you declare something like a [memoized function], it is associated with some jar.
By default, that jar is expected to be `crate::Jar`.
You can give the jar struct another name, or put it somewhere else, but then you will have to write `jar = path::to::your::Jar` everywhere, so it's not recommended.

Our `calc` example has only a single crate. We follow the salsa convention and declare the `Jar` struct at the root of the crate:

```rust
{{#include ../../../calc-example/calc/src/main.rs:jar_struct}}
```

You can see that a jar is just a tuple struct, but annotated with `#[salsa::Jar]`.
The fields of the struct correspond to the various things that need state in the database.
We're going to be introducing each of those fields through the tutorial.

[memoized functions]: ../reference/memoized.md
[entities]: ../reference/entity.md

The `salsa::jar` annotation also has a parameter, `db = Db`.
In general, salsa annotations take arguments of this form.
This particular argument is mandatory, so you'll get an error if you leave it out.
It identifies the **database trait** for this jar.

[^jar]: Jars of salsa -- get it? Get it??

## Database trait for the jar

Whereas a salsa jar contains all the storage needed for a particular crate,
the salsa **database** is a struct that contains all the storage needed for an entire program.
Typical salsa functions, however, don't refer directly to this database struct.
Instead, they refer to a trait, typically called `crate::Db`, that the final database must implement.
This allows for separate compilation, where you have a database that contains the data for two jars, but those jars don't depend on one another.

The database trait for our `calc` crate is very simple:

```rust
{{#include ../../../calc-example/calc/src/main.rs:jar_db}}
```

When you define a database trait like `Db`, the one thing that is required is that it must have a supertrait `salsa::DbWithJar<Jar>`,
where `Jar` is the jar struct. If your jar depends on other jars, you can have multiple such supertraits (e.g., `salsa::DbWithJar<other_crate::Jar>`).

Typically the `Db` trait has no other members or supertraits, but you are also free to add whatever other things you want in the trait.
When you define your final database, it will implement the trait, and you can then define the implementation of those other things.
This allows you to create a way for your jar to request context or other info from the database that is not moderated through salsa,
should you need that.

## Implementing the database trait for the jar

The `Db` trait must be implemented by the database struct.
We're going to define the database struct in a [later section](./db.md),
and one option would be to simply implement the jar `Db` trait there.
However, since we don't define any custom logic in the trait,
a common choice is to write a blanket impl for any type that implements `DbWithJar<Jar>`,
and that's what we do here:

```rust
{{#include ../../../calc-example/calc/src/main.rs:jar_db_impl}}
```

## Summary

If the concept of a jar seems a bit abstract to you, don't overthink it. The TL;DR is that when you create a salsa program, you need to do:

- In each of your crates:
  - Define a `#[salsa::jar(db = Db)]` struct, typically at `crate::Jar`, and list each of your various salsa-annotated things inside of it.
  - Define a `Db` trait, typically at `crate::Db`, that you will use in memoized functions and elsewhere to refer to the database struct.
- Once, typically in your final crate:
  - Define a database `D`, as described in the [next section](./db.md), that will contain a list of each of the jars for each of your crates.
  - Implement the `Db` traits for each jar for your database type `D` (often we do this through blanket impls in the jar crates).
