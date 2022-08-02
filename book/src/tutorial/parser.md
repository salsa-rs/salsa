# Defining the parser: memoized functions and inputs

The next step in the `calc` compiler is to define the parser.
The role of the parser will be to take the raw bytes from the input and create the `Statement`, `Function`, and `Expression` structures that [we defined in the `ir` module](./ir.md).

To minimize dependencies, we are going to write a [recursive descent parser][rd].
Another option would be to use a [Rust parsing framework](https://rustrepo.com/catalog/rust-parsing_newest_1).

[rd]: https://en.wikipedia.org/wiki/Recursive_descent_parser

## The `source_text` the function

Let's start by looking at the `source_text` function:

```rust
{{#include ../../../calc-example/calc/src/parser.rs:source_text}}
```

This is a bit of an odd function!
You can see it is annotated as memoized,
which means that salsa will store the return value in the database,
so that if you call it again, it does not re-execute unless its inputs have changed.
However, the function body itself is just a `panic!`, so it can never successfully return.
What is going on?

This function is an example of a common convention called an **input**.
Whenever you have a memoized function, it is possible to set its return value explicitly
(the [chapter on testing](./debug.md) shows how it is done).
When you set the return value explicitly, it never executes;
instead, when it is called, that return value is just returned.
This makes the function into an _input_ to the entire computation.

In this case, the body is just `panic!`,
which indicates that `source_text` is always meant to be set explicitly.
It's possible to set a return value for functions that have a body,
in which case they can act as either an input or a computation.

### Arguments to a memoized function

The first parameter to a memoized function is always the database,
which should be a `dyn Trait` value for the database trait associated with the jar
(the default jar is `crate::Jar`).

Memoized functions may take other arguments as well, though our examples here do not.
Those arguments must be something that can be interned.

### Memoized functions with `return_ref`

`source_text` is not only memoized, it is annotated with `return_ref`.
Ordinarily, when you call a memoized function,
the result you get back is cloned out of the database.
The `return_ref` attribute means that a reference into the database is returned instead.
So, when called, `source_text` will return an `&String` rather than cloning the `String`.
This is useful as a performance optimization.

## The `parse_statements` function

The next function is `parse_statements`, which has the job of actually doing the parsing.
The comments in the function explain how it works.

```rust
{{#include ../../../calc-example/calc/src/parser.rs:parse_statements}}
```

The most interesting part, from salsa's point of view,
is that `parse_statements` calls `source_text` to get its input.
Salsa will track this dependency.
If `parse_statements` is called again, it will only re-execute if the return value of `source_text` may have changed.

We won't explain how the parser works in detail here.
You can read the comments in the source file to get a better understanding.
But we will cover a few interesting points that interact with Salsa specifically.

### Creating interned values with the `from` method

The `parse_statement` method parses a single statement from the input:

```rust
{{#include ../../../calc-example/calc/src/parser.rs:parse_statement}}
```

The part we want to highlight is how an interned enum is created:

```rust
Statement::from(self.db, StatementData::Function(func))
```

On any interned value, the `from` method takes a database and an instance of the "data" type (here, `StatementData`).
It then interns this value and returns the interned type (here, `Statement`).

### Creating entity values, or interned structs, with the `new` method

The other way to create an interned/entity struct is with the `new` method.
This only works when the struct has named fields (i.e., it doesn't work with enums like `Statement`).
The `parse_function` method demonstrates:

```rust
{{#include ../../../calc-example/calc/src/parser.rs:parse_function}}
```

You can see that we invoke `FunctionnId::new` (an interned struct) and `Function::new` (an entity).
In each case, the `new` method takes the database, and then the value of each field.
