#![cfg(feature = "inventory")]

//! Test that a `tracked` fn on multiple salsa struct args
//! compiles and executes successfully.

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::interned]
struct MyInterned<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn salsa::Database, input: MyInput, interned: MyInterned<'db>) -> u32 {
    input.field(db) + interned.field(db)
}

#[test]
fn execute() {
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    let interned = MyInterned::new(&db, 33);
    assert_eq!(tracked_fn(&db, input, interned), 55);
}
