# Defining the parser: reporting errors

The last interesting case in the parser is how to handle a parse error.
Because Salsa functions are memoized and may not execute, they should not have side-effects,
so we don't just want to call `eprintln!`.
If we did so, the error would only be reported the first time the function was called, but not
on subsequent calls in the situation where the simply returns its memoized value.

Salsa defines a mechanism for managing this called an **accumulator**.
In our case, we define an accumulator struct called `Diagnostic` in the `ir` module:

```rust
{{#include ../../../examples/calc/ir.rs:diagnostic}}
```

Memoized functions can accumulate `Diagnostic` values.
Later, you can invoke a method to find all the values that were accumulated by the tracked functions
or any functions that they called
(e.g., we could get the set of `Diagnostic` values produced by the `parse_statements` function).

The `Parser::report_error` method contains an example of accumulating a diagnostic:

```rust
{{#include ../../../examples/calc/parser.rs:report_error}}
```

To get the diagnostics produced by `parse_statements`, or any other tracked function,
we invoke the associated `accumulated` function:

```rust
let accumulated: Vec<&Diagnostic> =
    parse_statements::accumulated::<Diagnostic>(db, source_program);
                      //            -----------
                      //     Use turbofish to specify
                      //     the diagnostics type.
```

`accumulated` takes the database followed by the tracked function's query arguments and returns a `Vec` of references.
