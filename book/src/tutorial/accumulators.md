# Defining the parser: reporting errors

The last interesting case in the parser is how to handle a parse error.
Because Salsa functions are memoized and may not execute, they should not have side-effects,
so we don't just want to call `eprintln!`.
If we did so, the error would only be reported the first time the function was called, but not
on subsequent calls in the situation where the simply returns its memoized value.

Salsa defines a mechanism for managing this called an **accumulator**.
In our case, we define an accumulator struct called `Diagnostics` in the `ir` module:

```rust
{{#include ../../../examples/calc/ir.rs:diagnostic}}
```

Accumulator structs are always newtype structs with a single field, in this case of type `Diagnostic`.
Memoized functions can _push_ `Diagnostic` values onto the accumulator.
Later, you can invoke a method to find all the values that were pushed by the memoized functions
or any functions that they called
(e.g., we could get the set of `Diagnostic` values produced by the `parse_statements` function).

The `Parser::report_error` method contains an example of pushing a diagnostic:

```rust
{{#include ../../../examples/calc/parser.rs:report_error}}
```

To get the set of diagnostics produced by `parse_errors`, or any other memoized function,
we invoke the associated `accumulated` function:

```rust
let accumulated: Vec<Diagnostic> =
    parse_statements::accumulated::<Diagnostics>(db);
                      //            -----------
                      //     Use turbofish to specify
                      //     the diagnostics type.
```

`accumulated` takes the database `db` as argument and returns a `Vec`.
