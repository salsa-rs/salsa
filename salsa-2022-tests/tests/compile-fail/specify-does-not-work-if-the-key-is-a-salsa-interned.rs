//! Test that `specify` does not work if the key is a `salsa::interned`
//! compilation fails
#![allow(warnings)]

#[salsa::jar(db = Db)]
struct Jar(MyInterned, MyTracked, tracked_fn);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::interned(jar = Jar)]
struct MyInterned {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
struct MyTracked {
    field: u32,
}

#[salsa::tracked(jar = Jar, specify)]
fn tracked_fn(db: &dyn Db, input: MyInterned) -> MyTracked {
    MyTracked::new(db, input.field(db) * 2)
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

fn main() {}
