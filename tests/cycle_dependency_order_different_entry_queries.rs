#![cfg(all(feature = "inventory", feature = "accumulator"))]

use crate::common::{ExecuteValidateLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{Database, Durability};

mod common;

#[salsa::input]
struct Input {
    stable: (),
}

#[salsa::tracked(cycle_initial=a_cycle_initial)]
fn query_a(db: &dyn Database, input: Input) {
    let _ = input.stable(db);
    let b = query_b(db, input);
    query_d(db, b);
}

fn a_cycle_initial(_db: &dyn Database, _id: salsa::Id, _input: Input) {}

#[salsa::interned]
struct Interned {
    value: u32,
}

#[salsa::tracked(cycle_initial=|db, _, _| Interned::new(db, 0))]
fn query_b(db: &dyn Database, input: Input) -> Interned<'_> {
    let _ = input.stable(db);
    query_c(db, input);
    Interned::new(db, 2)
}

#[salsa::tracked]
fn query_c(db: &dyn Database, input: Input) {
    let _ = input.stable(db);
    query_a(db, input);
}

#[salsa::tracked]
fn query_d<'db>(_db: &'db dyn Database, _i: Interned<'db>) {
    // reads some input
}

#[test_log::test]
fn the_test() {
    let mut db = ExecuteValidateLoggerDatabase::default();
    let input = Input::new(&db, ());

    // We compute the result starting from query a...
    query_a(&db, input);

    db.clear_logs();
    db.synthetic_write(Durability::HIGH);

    // ...but we now verify query_b
    query_b(&db, input);

    // What this test captures is that `Interned(Id(c00))` must be verified **before** `query_d(Id(c00))`
    // as we would when starting from `query_a`
    db.assert_logs(expect![[r#"
        [
            "salsa_event(WillExecute { database_key: query_b(Id(0)) })",
            "salsa_event(WillExecute { database_key: query_c(Id(0)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(400)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_d(Id(400)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_a(Id(0)) })",
        ]"#]]);
}
