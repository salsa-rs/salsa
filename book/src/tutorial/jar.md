# Jars and databases

Salsa programs are composed in **jars**[^jar].
A **jar** is the salsa version of a Rust crate or module.
It is a struct that contains the memoized results for some subset of your program.

Typically you define one jar per crate in your program, and you define it at the path `crate::Jar`.
You don't have to do this, but it's more convenient because the various salsa macros have defaults that expect the jar to be at this location.

Our `calc` example has only a single crate, but we'll still put the `Jar` struct at the root of the crate:

```rust
{{#include ../../../calc-example/calc/src/main.rs:jar_struct}}
```

The `#[salsa::jar]` annotation indicates that this struct is a Salsa jar.
The struct must be a tuple struct, and the fields in the struct correspond to the salsa [memoized functions], [entities], and other concepts that we are going to introduce in this tutorial.
The idea is that the field type will contain the storage needed to implement that particular salsa-ized thing.

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
Jars, however, don't refer directly to this database struct.
Instead, each jar defines a trait, typically called `Db`, that the struct must implement.
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
