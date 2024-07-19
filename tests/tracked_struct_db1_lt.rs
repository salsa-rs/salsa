//! Test that tracked structs with lifetimes not named `'db`
//! compile successfully.

mod common;

use test_log::test;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked1<'db1> {
    field: MyTracked2<'db1>,
}

#[salsa::tracked]
struct MyTracked2<'db2> {
    field: u32,
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[test]
fn create_db() {}
