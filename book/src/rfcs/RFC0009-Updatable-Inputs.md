# Updatable inputs

## Metadata

* Author: 1tgr
* Date: 2021-06-17
* Introduced in: https://github.com/salsa-rs/salsa/pull/1 (please update once you open your PR)

## Summary

- Update the value of an input query in place

## Motivation

- Adds an `update_some_input` function to input queries, which allows amendment of an input query's
  value without cloning it.

## User's guide

Input queries in Salsa are defined in terms of a pair of functions:

- `fn some_input(&self, key: u32) -> String`
- `fn set_some_input(&self, key: u32, value: String)`

Initial values are typically set when the database is instantiated, and are updated using a pattern
like this:

```rust
let mut value = db.some_input(123);
value.push_str("hello");
db.set_some_input(123, value);
```

The `some_input` function works by fetching the value from the input storage and returning a clone.
Cloning this value can be expensive.

This proposal exposes a third function, which can be used to update the value on an input query in
place, through a mutable reference to the underlying storage:

- `fn update_some_input<F>(&self, key: u32, value_fn: F) where F: FnOnce(&mut String)`

Under this proposal, the update pattern becomes:

```rust
db.update_some_input(|value: &mut String| {
    value.push_str("hello");
});
```

## Reference guide

We expose a `fn update` on the `InputQueryStorageOps` trait. The implementation of this function on
`InputStorage` requests a new revision and acquires a write lock (like `InputStorage::write`), but
it panics if the input query does not already have a value set (like `InputStorage::try_fetch`).

## Frequently asked questions

### What if the closure doesn't change the value?

Salsa is designed to invalidate downstream queries when an input has its value set, regardless of
whether the value changed. This proposal does not change this assumption, and a call like
`db.update_some_input(|_value| { /* do nothing */ })` will invalidate all queries that depend on
`some_input`.

### What happens if the closure panics?

Any updates made by the closure are visible; updates are not rolled back as the old value is no
longer available. Salsa's current revision number is incremented regardless, and any derived queries
are re-computed against the partially-updated value.
