# Plumbing

This chapter documents the code that salsa generates and its "inner workings".
We refer to this as the "plumbing".

This page walks through the ["Hello, World!"] example and explains the code that
it generates. Please take it with a grain of salt: while we make an effort to
keep this documentation up to date, this sort of thing can fall out of date
easily. See the page history below for major updates.

["Hello, World!"]: https://github.com/salsa-rs/salsa/blob/master/examples/hello_world/main.rs

If you'd like to see for yourself, you can set the environment variable
`SALSA_DUMP` to 1 while the procedural macro runs, and it will dump the full
output to stdout. I recommend piping the output through rustfmt.

## History

* 2020-07-05: Updated to take [RFC 6](rfcs/RFC0006-Dynamic-Databases.md) into account.
* 2020-06-24: Initial version.