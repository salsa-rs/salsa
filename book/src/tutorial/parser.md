# Defining the parser: memoized functions and inputs

The next step in the `calc` compiler is to define the parser.
The role of the parser will be to take the `SourceProgram` input,
read the string from the `text` field,
and create the `Statement`, `Function`, and `Expression` structures that [we defined in the `ir` module](./ir.md).

To minimize dependencies, we are going to write a [recursive descent parser][rd].
Another option would be to use a [Rust parsing framework](https://crates.io/categories/parsing).
We won't cover the parsing itself in this tutorial -- you can read the code if you want to see how it works.
We're going to focus only on the Salsa-related aspects.

[rd]: https://en.wikipedia.org/wiki/Recursive_descent_parser

## The `parse_statements` function

The starting point for the parser is the `parse_statements` function:

```rust
{{#include ../../../examples/calc/parser.rs:parse_statements}}
```

This function is annotated as `#[salsa::tracked]`.
That means that, when it is called, Salsa will track what inputs it reads as well as what value it returns.
The return value is _memoized_,
which means that if you call this function again without changing the inputs,
Salsa will reuse the result rather than re-execute it.

### Tracked functions are the unit of reuse

Tracked functions are the core part of how Salsa enables incremental reuse.
The goal of the framework is to avoid re-executing tracked functions and instead to reuse their result.
Salsa uses the [red-green algorithm](../reference/algorithm.md) to decide when to re-execute a function.
The short version is that a tracked function is re-executed if either (a) it directly reads an input, and that input has changed,
or (b) it directly invokes another tracked function and that function's return value has changed.
In the case of `parse_statements`, it directly reads `SourceProgram::text`, so if the text changes, then the parser will re-execute.

By choosing which functions to mark as `#[tracked]`, you control how much reuse you get.
In our case, we're opting to mark the outermost parsing function as tracked, but not the inner ones.
This means that if the input changes, we will always re-parse the entire input and re-create the resulting statements and so forth.
We'll see later that this _doesn't_ mean we will always re-run the type checker and other parts of the compiler.

This trade-off makes sense because (a) parsing is very cheap, so the overhead of tracking and enabling finer-grained reuse doesn't pay off
and because (b) since strings are just a big blob-o-bytes without any structure, it's rather hard to identify which parts of the IR need to be reparsed.
Some systems do choose to do more granular reparsing, often by doing a "first pass" over the string to give it a bit of structure,
e.g. to identify the functions,
but deferring the parsing of the body of each function until later.
Setting up a scheme like this is relatively easy in Salsa and uses the same principles that we will use later to avoid re-executing the type checker.

### Parameters to a tracked function

The **first** parameter to a tracked function is **always** the database, `db: &dyn crate::Db`.

Tracked functions may have no other parameters, one Salsa struct parameter, or multiple parameters that implement `Eq` and `Hash`.
A single Salsa struct can be used directly as the query key.
When a function has multiple parameters, Salsa interns their tuple to obtain a key, adding an interning step to each call.
Our `parse_statements` function takes one Salsa struct, the `SourceProgram` input.

### The `returns(copy)` annotation

You may have noticed that `parse_statements` is tagged with `#[salsa::tracked(returns(copy))]`.
Tracked functions ordinarily return a reference to their memoized value. The `returns(copy)`
attribute copies the value out of the database instead. This is a good fit for `Program`, which is
a small `Copy` handle to a tracked struct.

For return types that implement `Deref`, `returns(deref)` returns a reference to the dereferenced
target. For example, it can return a slice instead of a reference to a vector:

```rust
#[salsa::tracked(returns(deref))]
fn source_lines(db: &dyn crate::Db, source: SourceProgram) -> Vec<String> {
    source.text(db).lines().map(str::to_owned).collect()
}
```

Calling `source_lines` returns an `&[String]` rather than the default `&Vec<String>`.
That reference is tied to the database borrow and cannot be held across a new revision.
Use `returns(clone)` when returning an owned clone is more convenient and the clone is known to be
inexpensive.

(You may recall the `returns(deref)` annotation from the [IR](./ir.md) section of the tutorial,
where it was placed on struct fields. Return-mode annotations work the same way for fields and
tracked functions.)
