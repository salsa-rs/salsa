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

## The `parse_statements` function

The starting point for the parser is the `parse_statements` function:

```rust
{{#include ../../../examples/calc/parser.rs:parse_statements}}
```

This function is annotated as `#[salsa::tracked]`.
That means that, when it is called, Salsa will track what inputs it reads as well as what value it returns.
The return value is _memoized_,
which means that if you call this function again without changing the inputs,
Salsa will just clone the result rather than re-execute it.

### Tracked functions are the unit of reuse

Tracked functions are the core part of how Salsa enables incremental reuse.
The goal of the framework is to avoid re-executing tracked functions and instead to clone their result.
Salsa uses the [red-green algorithm](../reference/algorithm.md) to decide when to re-execute a function.
The short version is that a tracked function is re-executed if either (a) it directly reads an input, and that input has changed,
or (b) it directly invokes another tracked function and that function's return value has changed.
In the case of `parse_statements`, it directly reads `ProgramSource::text`, so if the text changes, then the parser will re-execute.

By choosing which functions to mark as `#[tracked]`, you control how much reuse you get.
In our case, we're opting to mark the outermost parsing function as tracked, but not the inner ones.
This means that if the input changes, we will always re-parse the entire input and re-create the resulting statements and so forth.
We'll see later that this _doesn't_ mean we will always re-run the type checker and other parts of the compiler.

This trade-off makes sense because (a) parsing is very cheap, so the overhead of tracking and enabling finer-grained reuse doesn't pay off
and because (b) since strings are just a big blob-of-bytes without any structure, it's rather hard to identify which parts of the IR need to be reparsed.
Some systems do choose to do more granular reparsing, often by doing a "first pass" over the string to give it a bit of structure,
e.g. to identify the functions,
but deferring the parsing of the body of each function until later.
Setting up a scheme like this is relatively easy in Salsa and uses the same principles that we will use later to avoid re-executing the type checker.

### Parameters to a tracked function

The **first** parameter to a tracked function is **always** the database, `db: &dyn crate::Db`.

The **second** parameter to a tracked function is **always** some kind of Salsa struct.

Tracked functions may take other arguments as well, though our examples here do not.
Functions that take additional arguments are less efficient and flexible.
It's generally better to structure tracked functions as functions of a single Salsa struct if possible.


## The `returns` attribute for functions

You may have noticed that `parse_statements` is tagged with `#[salsa::tracked]`.
Ordinarily, when you call a tracked function, the result you get back
**is cloned** out of the database.

You may recall the various `returns` annotations for struct fields from the
[IR](./ir.md#the-returns-attribute-for-struct-fields) chapter. Those same
annotations can be used for salsa functions with roughly the same meaning as in
struct fields.

- `salsa::tracked(returns(clone))` (**the default**): Invokes `Clone::clone` on function's return type.
- `salsa::tracked(returns(ref))`: Returns a reference to the functions return type: `&Type` .
- `salsa::tracked(returns(deref))`: Invokes `Deref::deref` on the return type.
- `salsa::tracked(returns(copy))`: Returns an owned copy of the value.

We will use a modified version of the example from the [IR](./ir.md#the-returns-attribute-for-struct-fields)
chapter to explain how the `returns` annotation works for function return types
instead of struct fields.

```rust
/// Number wraps an i32 and is Copy.
#[derive(PartialEq, Eq, Copy, Debug)]
struct Number(i32);

// Dummy clone implementation that logs the Clone::clone call.
impl Clone for Number {
    fn clone(&self) -> Self {
        println!("Cloning {self:?}...");
        Number(self.0)
    }
}

// Deref into the wrapped i32 and log the call.
impl std::ops::Deref for Number {
    type Target = i32;

    fn deref(&self) -> &Self::Target {
        println!("Dereferencing {self:?}...");
        &self.0
    }
}

// Salsa struct.
#[salsa::input]
struct Input {
    number: Number,
}

/// Salsa database to use in our example.
#[salsa::db]
#[derive(Clone, Default)]
struct NumDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for NumDb {}
```

Now we'll add a simple salsa tracked function:

```rust
#[salsa::tracked]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

And a simple program that makes use of the `number` function:

```rust
let db: NumDb = Default::default();
let input = Input::new(&db, Number(42));

// Call the salsa::tracked number function.
let n = number(&db, input);
eprintln!("n: {n:?}");
```

### `#[salsa::tracked(returns(clone))]` (default)

By default, if we only add `salsa::tracked` to our function it will clone the
value before returning it. So, with this function signature and salsa struct:

```rust
#[salsa::input]
struct Input {
    number: Number,
}

#[salsa::tracked]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

The output of our program is:

```
Cloning Number(42)...
Cloning Number(42)...
n: Number(42)
```

And the `type` of the `n` variable is:

```rust
let n: Number = number(&db, input);
```

Note that the value is cloned twice. One of the comes from accessing the field
inside the salsa function (`input.number(&db)`) and the other one comes from
calling the salsa function itself (`let n = number(&db)`).

Explicit annotation looks like this:

```rust
#[salsa::tracked(returns(clone))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

### `#[salsa::tracked(returns(ref))]`

The `returns(ref)` annotation, just like with struct fields, will not call
`Clone::clone` and return a ref instead. Given this salsa struct and function:

```rust
#[salsa::input]
struct Input {
    number: Number,
}

#[salsa::tracked(returns(ref))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

The output of our program is:

```
Cloning Number(42)...
n: Number(42)
```

And the `type` of the `n` variable is:

```rust
let n: &Number = number(&db, input);
```

Accessing the field (`input.number(&db)`) makes a clone and then the `number`
function returns a reference to that clone. To completely avoid clones, we can
mark the struct field as `returns(copy)`:

```rust
#[salsa::input]
struct Input {
    #[returns(copy)]
    number: Number,
}

#[salsa::tracked(returns(ref))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

Output:

```
n: Number(42)
```

We chose `returns(copy)` over `returns(ref)` for the field because having `ref` on
both the field and the function return type would not work. The call to
`input.number(&db)` would return a `&Number` and we can't return references from
salsa functions ourselves because the lifetimes are managed by the salsa macro
generated code to make sure they are valid across database revisions.

The general use case for `tracked` functions isn't to track "references" but
*to transform* data, for example transforming an AST structure into an IR
structure. So usually you will not be accessing references just to return the
same references in tracked functions.

### `#[salsa::tracked(returns(deref))]`

This annotation simply calls `Deref::deref` on the returned type. Given these
salsa items:

```rust
#[salsa::input]
struct Input {
    number: Number,
}

#[salsa::tracked(returns(deref))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

Our program prints:

```
Cloning Number(42)...
Dereferencing Number(42)...
n: 42
```

And the type of `n` is:

```rust
let n: &i32 = number(&db, input);
```

The clone can be removed again by marking the number field as `returns(copy)`.

### `#[salsa::tracked(returns(copy))]`

If the return type is `Copy` then we can mark the function with `returns(copy)`.
Given this case:

```rust
#[salsa::input]
struct Input {
    number: Number,
}

#[salsa::tracked(returns(copy))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

The output of our program is:

```
Cloning Number(42)...
n: Number(42)
```

And the type of `n` is:

```rust
let n: Number = number(&db, input);
```

The clone can be removed by marking the `number` as `returns(copy)` as well.

