//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked<'_>, tracked_fn);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
    MyTracked::new(db, input.field(db) * 2)
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
fn execute() {
    let db = Database::default();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input).field(&db), 44);
}
