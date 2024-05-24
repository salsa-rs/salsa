//! Test that `specify` does not work if the key is a `salsa::interned`
//! compilation fails
#![allow(warnings)]

#[salsa::jar(db = Db)]
struct Jar(MyInterned<'_>, MyTracked<'_>, tracked_fn);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::interned(jar = Jar)]
struct MyInterned<'db> {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked(jar = Jar, specify)]
fn tracked_fn<'db>(db: &'db dyn Db, input: MyInterned<'db>) -> MyTracked<'db> {
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
