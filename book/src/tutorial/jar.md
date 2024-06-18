# Jars and databases

Before we can define the interesting parts of our Salsa program, we have to setup a bit of structure that defines the Salsa **database**.
The database is a struct that ultimately stores all of Salsa's intermediate state, such as the memoized return values from [tracked functions].

[tracked functions]: ../overview.md#tracked-functions

The database itself is defined in terms of intermediate structures, called **jars**[^jar], which themselves contain the data for each function.
This setup allows Salsa programs to be divided amongst many crates.
Typically, you define one jar struct per crate, and then when you construct the final database, you simply list the jar structs.
This permits the crates to define private functions and other things that are members of the jar struct, but not known directly to the database.

[^jar]: Jars of salsa -- get it? Get it??[^java]

[^java]: OK, maybe it also brings to mind Java `.jar` files, but there's no real relationship. A jar is just a Rust struct, not a packaging format.

## Defining a jar struct

To define a jar struct, you create a tuple struct with the `#[salsa::jar]` annotation:

```rust
{{#include ../../../examples/calc/main.rs:jar_struct}}
```

Although it's not required, it's highly recommended to put the `jar` struct at the root of your crate, so that it can be referred to as `crate::Jar`.
All of the other Salsa annotations reference a jar struct, and they all default to the path `crate::Jar`.
If you put the jar somewhere else, you will have to override that default.

## Defining the database trait

The `#[salsa::jar]` annotation also includes a `db = Db` field.
The value of this field (normally `Db`) is the name of a trait that represents the database.
Salsa programs never refer _directly_ to the database; instead, they take a `&dyn Db` argument.
This allows for separate compilation, where you have a database that contains the data for two jars, but those jars don't depend on one another.

The database trait for our `calc` crate is very simple:

```rust
{{#include ../../../examples/calc/main.rs:jar_db}}
```

When you define a database trait like `Db`, the one thing that is required is that it must have a supertrait `salsa::DbWithJar<Jar>`,
where `Jar` is the jar struct. If your jar depends on other jars, you can have multiple such supertraits (e.g., `salsa::DbWithJar<other_crate::Jar>`).

Typically the `Db` trait has no other members or supertraits, but you are also free to add whatever other things you want in the trait.
When you define your final database, it will implement the trait, and you can then define the implementation of those other things.
This allows you to create a way for your jar to request context or other info from the database that is not moderated through Salsa,
should you need that.

## Implementing the database trait for the jar

The `Db` trait must be implemented by the database struct.
We're going to define the database struct in a [later section](./db.md),
and one option would be to simply implement the jar `Db` trait there.
However, since we don't define any custom logic in the trait,
a common choice is to write a blanket impl for any type that implements `DbWithJar<Jar>`,
and that's what we do here:

```rust
{{#include ../../../examples/calc/main.rs:jar_db_impl}}
```

## Summary

If the concept of a jar seems a bit abstract to you, don't overthink it. The TL;DR is that when you create a Salsa program, you need to perform the following steps:

- In each of your crates:
  - Define a `#[salsa::jar(db = Db)]` struct, typically at `crate::Jar`, and list each of your various Salsa-annotated things inside of it.
  - Define a `Db` trait, typically at `crate::Db`, that you will use in memoized functions and elsewhere to refer to the database struct.
- Once, typically in your final crate:
  - Define a database `D`, as described in the [next section](./db.md), that will contain a list of each of the jars for each of your crates.
  - Implement the `Db` traits for each jar for your database type `D` (often we do this through blanket impls in the jar crates).
