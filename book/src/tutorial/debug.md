# Defining the parser: debug impls and testing

As the final part of the parser, we need to write some tests.
To do so, we will create a database, set the input source text, run the parser, and check the result.
Before we can do that, though, we have to address one question: how do we inspect the value of an interned type like `Expression`?

## The `DebugWithDb` trait

Because an interned type like `Expression` just stores an integer, the traditional `Debug` trait is not very useful.
To properly print a `Expression`, you need to access the salsa database to find out what its value is.
To solve this, `salsa` provides a `DebugWithDb` trait that acts like the regular `Debug`, but takes a database as argument.
For types that implement this trait, you can invoke the `debug` method.
This returns a temporary that implements the ordinary `Debug` trait, allowing you to write something like

```rust
eprintln!("Expression = {:?}", expr.debug(db));
```

and get back the output you expect.

## Implementing the `DebugWithDb` trait

For now, unfortunately, you have to implement the `DebugWithDb` trait manually, as we do not provide a derive.
This is tedious but not difficult. Here is an example of implementing the trait for `Expression`:

```rust
{{#include ../../../calc-example/calc/src/ir.rs:expression_debug_impl}}
```

Some things to note:

- The `data` method gives access to the full enum from the database.
- The [`Formatter`] methods (e.g., [`debug_tuple`]) can be used to provide consistent output.
- When printing the value of a field, use `.field(&a.debug(db))` for fields that are themselves interned or entities, and use `.field(&a)` for fields that just implement the ordinary `Debug` trait.

[`debug_tuple`]: https://doc.rust-lang.org/std/fmt/struct.Formatter.html#method.debug_tuple
[`formatter`]: https://doc.rust-lang.org/std/fmt/struct.Formatter.html#

## Forwarding to the ordinary `Debug` trait

For consistency, it is sometimes useful to have a `DebugWithDb` implementation even for types, like `Op`, that are just ordinary enums. You can do that like so:

```rust
{{#include ../../../calc-example/calc/src/ir.rs:op_debug_impl}}
```

## Writing the unit test

Now that we have our `DebugWithDb` impls in place, we can write a simple unit test harness.
The `parse_string` function below creates a database, sets the source text, and then invokes the parser:

```rust
{{#include ../../../calc-example/calc/src/parser.rs:parse_string}}
```

Combined with the [`expect-test`](https://crates.io/crates/expect-test) crate, we can then write unit tests like this one:

```rust
{{#include ../../../calc-example/calc/src/parser.rs:parse_print}}
```
