//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.

use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked<'_>, MyInterned<'_>);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[salsa::input]
struct MyInput {
    field: String,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: MyInterned<'db>,
}

#[salsa::interned]
struct MyInterned<'db> {
    field: String,
}

#[test]
fn execute() {}
