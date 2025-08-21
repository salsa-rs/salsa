#![cfg(feature = "inventory")]

//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use salsa::{Database, Setter};
use test_log::test;

#[salsa::input]
struct Input {
    #[returns(ref)]
    field1: Box<usize>,
}

#[salsa::interned]
struct Interned<'db> {
    field1: &'db Box<usize>,
}

#[test]
fn test() {
    #[salsa::tracked]
    fn intern<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        Interned::new(db, input.field1(db))
    }

    let mut db = common::LoggerDatabase::default();

    let input = Input::new(&db, Box::new(100));

    let _interned = intern(&db, input);

    input.set_field1(&mut db).to(Box::new(100));

    let interned = intern(&db, input);
    dbg!(interned.field1(&db));
}
