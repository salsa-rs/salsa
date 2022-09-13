# Defining the parser: memoized functions and inputs

The next step in the `calc` compiler is to define the parser.
The role of the parser will be to take the `ProgramSource` input,
read the string from the `text` field,
and create the `Statement`, `Function`, and `Expression` structures that [we defined in the `ir` module](./ir.md).

To minimize dependencies, we are going to write a [recursive descent parser][rd].
Another option would be to use a [Rust parsing framework](https://rustrepo.com/catalog/rust-parsing_newest_1).
We won't cover the parsing itself in this tutorial -- you can read the code if you want to see how it works.
We're going to focus only on the Salsa-related aspects.

[rd]: https://en.wikipedia.org/wiki/Recursive_descent_parser

## The `parse_source_program` function

The starting point for the parser is the `parse_source_program` function:

```rust
{{#include ../../../calc-example/calc/src/parser.rs:parse_source_program}}
```

This function is annotated as `#[salsa::tracked]`.
That means that, when it is called, Salsa will track what inputs it reads as well as what value it returns.
The return value is *memoized*,
which means that if you call this function again without changing the inputs,
Salsa will just clone the result rather than re-execute it.
(Because the result in this case is a `Program`, which is a tracked struct, cloning is very cheap.)

### Tracked functions are the unit of reuse

Tracked functions are the core part of how Salsa enables incremental reuse.
The goal of the framework is to avoid re-executing tracked functions and instead to clone their result.
Salsa uses the [red-green algorithm](../reference/algorithm.md) to decide when to re-execute a function.
The short version is that a tracked function is re-executed if either (a) it directly reads an input, and that input has changed,
or (b) it directly invokes another tracked function and that function's return value has changed.
In the case of `parse_statements`, it reads the field `source.text(db)`, so if the text changes, then the parser will re-execute.

By choosing which functions to mark as `#[tracked]`, you control how much reuse you get.
In our case, we're opting to mark the outermost parsing function as tracked, but not the inner ones.
This means that if the input changes, we will always re-parse the entire input and re-create the resulting statements and so forth.
We'll see later that this *doesn't* mean we will always re-run the type checker and other parts of the compiler.

This trade-off makes sense because (a) parsing is very cheap, so the overhead of tracking and enabling finer-grained reuse doesn't pay off
and because (b) since strings are just a big blob-o-bytes without any structure, it's rather hard to identify which parts of the IR need to be reparsed.
Some systems do choose to do more granular reparsing, often by doing a "first pass" over the string to give it a bit of structure, 
e.g. to identify the functions,
but deferring the parsing of the body of each function until later.
Setting up a scheme like this is relatively easy in Salsa and uses the same principles that we will use later to avoid re-executing the type checker.

### Parameters to a tracked function

The **first** parameter to a tracked function is **always** the database, `db: &dyn crate::Db`.
It must be a `dyn` value of whatever database is associated with the jar.

The **second** parameter to a tracked function is **always** some kind of Salsa struct.
The first parameter to a memoized function is always the database,
which should be a `dyn Trait` value for the database trait associated with the jar
(the default jar is `crate::Jar`).

Tracked functions may take other arguments as well, though our examples here do not.
Functions that take additional arguments are less efficient and flexible.
It's generally better to structure tracked functions as functions of a single Salsa struct if possible.

