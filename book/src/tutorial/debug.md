# Defining the parser: debug impls and testing

As the final part of the parser, we need to write some tests.
To do so, we will create a database, set the input source text, run the parser, and check the result.
Before we can do that, though, we have to address one question: how do we inspect Salsa structs whose fields live in the database?

## Generated `Debug` implementations

The `debug` option on Salsa struct attributes generates an ordinary `Debug` implementation.
Tracked functions attach the database automatically; other code can use `salsa::Database::attach` to include field values:

```rust
use salsa::Database as _;

db.attach(|db| {
    let function = FunctionId::new(db, "area_circle".to_string());
    eprintln!("Function = {function:?}");
});
```

Without an attached database, the generated formatter displays only the Salsa ID.

## Debug for ordinary Rust types

Types such as `Op` that are not Salsa structs can use the ordinary `Debug` derive:

```rust
#[derive(Debug)]
pub enum Op {
    Add, Subtract, Multiply, Divide,
}
```

## Writing the unit test

Now that we have our `Debug` implementations in place, we can write a simple unit test harness.
The `parse_string` function below creates a database, sets the source text, and then invokes the parser:

```rust
{{#include ../../../examples/calc/parser.rs:parse_string}}
```

Combined with the [`expect-test`](https://crates.io/crates/expect-test) crate, we can then write unit tests like this one:

```rust
{{#include ../../../examples/calc/parser.rs:parse_print}}
```
