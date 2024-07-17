//! Test that `specify` only works if the key is a tracked struct created in the current query.
//! compilation succeeds but execution panics
#![allow(warnings)]

#[salsa::db]
trait Db: salsa::Database {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_struct_created_in_another_query<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
    MyTracked::new(db, input.field(db) * 2)
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
    let t = tracked_struct_created_in_another_query(db, input);
    if input.field(db) != 0 {
        tracked_fn_extra::specify(db, t, 2222);
    }
    t
}

#[salsa::tracked(specify)]
fn tracked_fn_extra<'db>(_db: &'db dyn Db, _input: MyTracked<'db>) -> u32 {
    0
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

#[test]
#[should_panic(
    expected = "can only use `specify` on salsa structs created during the current tracked fn"
)]
fn execute_when_specified() {
    let mut db = Database::default();
    let input = MyInput::new(&db, 22);
    let tracked = tracked_fn(&db, input);
}
