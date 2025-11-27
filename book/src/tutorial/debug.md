# Defining the parser: debug impls and testing

As the final part of the parser, we need to write some tests.
To do so, we will create a database, set the input source text, run the parser, and check the result.
Before we can do that, though, we have to address one question: how do we inspect the value of a `salsa::tracked`
type like `Program`?

## The `salsa::Database::attach` method

Because a tracked type like `Program` just stores an integer, the traditional `Debug` trait is not very useful.
To properly print a `Program`, you need to access the Salsa database to find out what its value is.
To solve this, `salsa` provides the `debug` option when declaring tracked structs: `#[salsa::tracked(debug)]` which
creates an implementation of `Debug` that can access the values of `Program` in
the salsa database:

```rust
{{#include ../../../examples/calc/ir.rs:program}}
```

Specifying the `debug` option allows us to use our types in formatted strings,
but it's not enough to get the full value. Simply writing this code:

```rust
use db::CalcDatabaseImpl;
use ir::SourceProgram;
use parser::parse_statements;

let db: CalcDatabaseImpl = Default::default();

let surce_text = "print 1 + 2";
let source_program = SourceProgram::new(db, source_text.to_string());

let statements = parse_statements(db, source_program);

println!("{:#?}", statements);
```

gives us this output:

```
Program {
    [salsa id]: Id(800),
}
```

And that is because when `println!` calls `Debug::fmt` on our `statements` variable
of type `Program`, the `Debug::fmt` implementation has no access to the Salsa database
to inspect the values.

In order to allow `Debug::fmt` to access the database, we can call it inside a
function passed to `Database::attach` which sets a thread-local variable to the
`Database` value it was called on, allowing the debug implementation to access it
and inspect values:

```rust
use db::CalcDatabaseImpl;
use ir::SourceProgram;
use parser::parse_statements;

let db: CalcDatabaseImpl = Default::default();

db.attach(|db| {
    let surce_text = "print 1 + 2";
    let source_program = SourceProgram::new(db, source_text.to_string());

    let statements = parse_statements(db, source_program);

    println!("{:#?}", statements);
})
```

Now we should see something like this:

```
Program {
    [salsa id]: Id(800),
    statements: [
        Statement {
            span: Span {
                [salsa id]: Id(404),
                start: 0,
                end: 11,
            },
            data: Print(
                Expression {
                    span: Span {
                        [salsa id]: Id(403),
                        start: 6,
                        end: 11,
                    },
                    data: Op(
                        Expression {
                            span: Span {
                                [salsa id]: Id(400),
                                start: 6,
                                end: 7,
                            },
                            data: Number(
                                1.0,
                            ),
                        },
                        Add,
                        Expression {
                            span: Span {
                                [salsa id]: Id(402),
                                start: 10,
                                end: 11,
                            },
                            data: Number(
                                2.0,
                            ),
                        },
                    ),
                },
            ),
        },
    ],
}
```

## Writing the unit test

Now that we know how to inspect all the values of Salsa structs, we can write a simple unit test harness.
The `parse_string` function below creates a database, sets the source text, and then invokes the parser
to get the statements and creates a formatted string with all the values:

```rust
{{#include ../../../examples/calc/parser.rs:parse_string}}
```

Combined with the [`expect-test`](https://crates.io/crates/expect-test) crate, we can then write unit tests like this one:

```rust
{{#include ../../../examples/calc/parser.rs:parse_print}}
```
