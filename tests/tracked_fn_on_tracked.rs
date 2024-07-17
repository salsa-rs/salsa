//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyTracked<'db> {
    MyTracked::new(db, input.field(db) * 2)
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[test]
fn execute() {
    let db = Database::default();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input).field(&db), 44);
}
