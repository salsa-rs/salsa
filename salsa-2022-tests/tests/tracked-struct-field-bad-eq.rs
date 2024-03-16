//! Test a field whose `PartialEq` impl is always true.
//! This can our "last changed" data to be wrong
//! but we *should* always reflect the final values.

use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked, the_fn);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input]
struct MyInput {
    field: bool,
}

#[derive(Eq, Hash, Debug, Clone)]
struct BadEq {
    field: bool,
}

impl PartialEq for BadEq {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl From<bool> for BadEq {
    fn from(value: bool) -> Self {
        Self { field: value }
    }
}

#[salsa::tracked]
struct MyTracked {
    #[id]
    field: BadEq,
}

#[salsa::tracked]
fn the_fn(db: &dyn Db, input: MyInput) {
    let tracked0 = MyTracked::new(db, BadEq::from(input.field(db)));
    assert_eq!(tracked0.field(db).field, input.field(db));
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
    let mut db = Database::default();

    let input = MyInput::new(&db, true);
    the_fn(&db, input);
    input.set_field(&mut db).to(false);
    the_fn(&db, input);
}
