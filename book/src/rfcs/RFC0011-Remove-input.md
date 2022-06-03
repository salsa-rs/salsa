# Removeable inputs

## Metadata

* Author: 1tgr, nikomatsakis
* Date: 2021-06-17
* Introduced in: https://github.com/salsa-rs/salsa/pull/275

## Summary

- Remove the value of an input query completely

## Motivation

Adds an `remove_input` function to input queries, which allows removing (and taking ownership of) the value of a given key. This permits the value to be modified and then re-inserted without cloning.

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

We expose a `fn remove` on the `InputQueryStorageOps` trait. The implementation of this function on
`InputStorage` requests a new revision, acquires a write lock (like `InputStorage::write`), and
then removes and returns the key. Any subsequent attempt to read that key will panic.

Note that the slot id remains untouched.

## Frequently asked questions

### What about giving a closure to update in place?

This RFC evolved as an alternative to https://github.com/salsa-rs/salsa/pull/273, which proposed an `update` method that took a closure. The problem with that approach is that it raises some thorny questions -- e.g., what happens if the closure panics? This "take and re-insert" strategy is infallible and cleaner.
