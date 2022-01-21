# Generated code

This page walks through the ["Hello, World!"] example and explains the code that
it generates. Please take it with a grain of salt: while we make an effort to
keep this documentation up to date, this sort of thing can fall out of date
easily. See the page history below for major updates.

["Hello, World!"]: https://github.com/salsa-rs/salsa/blob/master/examples/hello_world/main.rs

If you'd like to see for yourself, you can set the environment variable
`SALSA_DUMP` to 1 while the procedural macro runs, and it will dump the full
output to stdout. I recommend piping the output through rustfmt.

## Sources

The main parts of the source that we are focused on are as follows.

### Query group

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:trait}}
```

### Database

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:database}}
```
