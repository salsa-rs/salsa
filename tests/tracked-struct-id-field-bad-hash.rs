//! Test for a tracked struct where the id field has a
//! very poorly chosen hash impl (always returns 0).
//! This demonstrates that the `#[id]` fields on a struct
//! can change values and yet the struct can have the same
//! id (because struct ids are based on the *hash* of the
//! `#[id]` fields).

use salsa::{Database as Db, Setter};
use test_log::test;

#[salsa::input]
struct MyInput {
    field: bool,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
struct BadHash {
    field: bool,
}

impl From<bool> for BadHash {
    fn from(value: bool) -> Self {
        Self { field: value }
    }
}

impl std::hash::Hash for BadHash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_i16(0);
    }
}

#[salsa::tracked]
struct MyTracked<'db> {
    #[id]
    field: BadHash,
}

#[salsa::tracked]
fn the_fn(db: &dyn Db, input: MyInput) {
    let tracked0 = MyTracked::new(db, BadHash::from(input.field(db)));
    assert_eq!(tracked0.field(db).field, input.field(db));
}

#[test]
fn execute() {
    let mut db = salsa::DatabaseImpl::new();

    let input = MyInput::new(&db, true);
    the_fn(&db, input);
    input.set_field(&mut db).to(false);
    the_fn(&db, input);
}
