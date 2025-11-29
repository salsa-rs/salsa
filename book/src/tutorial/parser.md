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


## The `returns` attribute

You may have noticed that `parse_statements` is tagged with `#[salsa::tracked]`.
Ordinarily, when you call a tracked function, the result you get back
**is cloned** out of the database.

### `returns(ref)`

If we wanted to avoid cloning the result, then we could use the `returns(ref)`
attribute like this:

```rust
#[salsa::tracked(returns(ref))]
pub fn parse_statements(db: &dyn crate::Db, source: SourceProgram) -> Program<'_>
```

The `returns(ref)` attribute means that a reference into the database is returned
instead of a clone. So, when called, `parse_statements` would return a
`&Vec<Statement>` rather than cloning the `Vec`. This is useful as a performance
optimization. (You may recall the `returns(ref)` annotation from the [ir](./ir.md)
section of the tutorial, where it was placed on struct fields, with roughly the
same meaning.)

### `returns(copy)`

To illustrate how `returns(copy)` works, we'll use this simple example:

```rust
// Note that Number is Copy.
#[derive(PartialEq, Eq, Copy)]
struct Number(i32);

// Dummy clone that simulates expensive memory operations.
impl Clone for Number {
    fn clone(&self) -> Number {
        println!("Very expensive clone here...");
        Number(self.0)
    }
}

// Salsa input wraps the Number struct above.
#[salsa::input]
struct Input {
    number: Number,
}

// The number function "unwraps" the Number from the salsa input.
#[salsa::tracked]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

Now let's define the Salsa database:

```rust
#[salsa::db]
#[derive(Clone, Default)]
struct Db {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Db {}
```

Note that we did not use the `returns` attribute anywhere. So if we run this code:

```rust
let db: Db = Default::default();
let input = Input::new(&db, Number(42));

let n = number(&db, input);
println!("number: {:?}", n.0);
```

we get this output:

```
Very expensive clone here...
Very expensive clone here...
number: 42
```

That's because the default behavior is cloning as we mentioned before. If we
make a little change and annotate the `number` function with `returns(copy)`:

```rust
#[salsa::tracked(returns(copy))]
fn number(db: &dyn salsa::Database, input: Input) -> Number
```

then we get this output:

```
Very expensive clone here...
number: 42
```

One of the clones is gone, the `number` function is no longer invoking `Clone::clone` before returning the result. But where is the other clone
coming from?

It's coming from the `Input::number` accessor, this line also clones:

```rust
#[salsa::tracked(returns(copy))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db) // This function also calls Clone::clone
}
```

To get rid of that clone as well we need to also annotate the `number` field as
`returns(copy)`:

```rust
#[salsa::input]
struct Input {
    #[returns(copy)]
    number: Number,
}
```

And now `Clone::clone` is no longer called anywhere. This is the output:

```
number: 42
```

### `returns(deref)`

The `returns(deref)` attribute simply calls `Deref::deref` on the result and
returns that instead. Let's make some changes to the `Number` example above to
see how it works:

```rust
#[derive(PartialEq, Eq)]
struct Number(i32);

// Number now implements deref and targets the underlying i32 type.
impl std::ops::Deref for Number {
    type Target = i32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[salsa::input]
struct Input {
    number: Number,
}

// Annotate with deref return.
#[salsa::tracked(returns(deref))]
fn number(db: &dyn salsa::Database, input: Input) -> Number {
    input.number(db)
}
```

Now, when calling `number`, even though the the signature is `fn -> Number` we
will get an `&i32` type instead (the deref `&Target`). So our code needs to
change to use the new type:

```rust
let db: Db = Default::default();
let input = Input::new(&db, Number(42));

let n = number(&db, input);
eprintln!("number: {:?}", n); // Note that n.0 no longer works, n is &i32
```

Similarly, we can annotate the field of the salsa stuct as `returns(deref)`, in which case we could change the signature of our `number` function to this:

```rust
#[salsa::tracked]
fn number(db: &dyn salsa::Database, input: Input) -> i32 {
    // This getter now returns &i32, we can deref it ourselves.
    *input.number(db)
}
```