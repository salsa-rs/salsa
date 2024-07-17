//! Test a field whose `PartialEq` impl is always true.
//! This can our "last changed" data to be wrong
//! but we *should* always reflect the final values.

use salsa::{Database as Db, Setter};
use test_log::test;

#[salsa::input]
struct MyInput {
    field: bool,
}

#[derive(Hash, Debug, Clone)]
struct NotEq {
    field: bool,
}

impl From<bool> for NotEq {
    fn from(value: bool) -> Self {
        Self { field: value }
    }
}

#[salsa::tracked]
struct MyTracked<'db> {
    #[no_eq]
    field: NotEq,
}

#[salsa::tracked]
fn the_fn(db: &dyn Db, input: MyInput) {
    let tracked0 = MyTracked::new(db, NotEq::from(input.field(db)));
    assert_eq!(tracked0.field(db).field, input.field(db));
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
    let mut db = Database::default();

    let input = MyInput::new(&db, true);
    the_fn(&db, input);
    input.set_field(&mut db).to(false);
    the_fn(&db, input);
}
