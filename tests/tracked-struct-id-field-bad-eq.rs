//! Test an id field whose `PartialEq` impl is always true.

use salsa::{Database as Db, Setter};
use test_log::test;

#[salsa::input]
struct MyInput {
    field: bool,
}

#[allow(clippy::derived_hash_with_manual_eq)]
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
struct MyTracked<'db> {
    #[id]
    field: BadEq,
}

#[salsa::tracked]
fn the_fn(db: &dyn Db, input: MyInput) {
    let tracked0 = MyTracked::new(db, BadEq::from(input.field(db)));
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
