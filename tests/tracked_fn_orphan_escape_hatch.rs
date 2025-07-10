#![cfg(feature = "inventory")]

//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
#![allow(warnings)]

use std::marker::PhantomData;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotUpdate<'a>(PhantomData<fn() -> &'a ()>);

#[salsa::tracked(unsafe(non_update_return_type))]
fn tracked_fn(db: &dyn salsa::Database, input: MyInput) -> NotUpdate<'_> {
    NotUpdate(PhantomData)
}

#[test]
fn execute() {
    let mut db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    tracked_fn(&db, input);
}
