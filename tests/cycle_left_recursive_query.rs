#![cfg(all(feature = "inventory", feature = "accumulator"))]

use crate::common::{ExecuteValidateLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{Database, Durability, Id};

mod common;

#[salsa::input]
struct Input {
    stable: (),
}

#[salsa::tracked(cycle_initial=cycle_initial)]
fn query_a(db: &dyn salsa::Database, input: Input) -> Interned<'_> {
    let _ = input.stable(db);
    let interned = query_b(db, input);
    let value = interned.value(db);

    if value < 10 {
        Interned::new(db, value + 1)
    } else {
        interned
    }
}

#[salsa::tracked]
fn query_b(db: &dyn Database, input: Input) -> Interned<'_> {
    let _ = input.stable(db);
    let interned = query_a(db, input);
    query_x(db, interned);
    interned
}

#[salsa::tracked]
fn query_x<'db>(_db: &'db dyn Database, _i: Interned<'db>) {}

fn cycle_initial(db: &dyn Database, _id: Id, _input: Input) -> Interned<'_> {
    Interned::new(db, 0)
}

#[salsa::interned]
struct Interned {
    value: u32,
}

#[test_log::test]
fn the_test() {
    let mut db = ExecuteValidateLoggerDatabase::default();
    let input = Input::new(&db, ());

    let result = query_a(&db, input);

    assert_eq!(result.value(&db), 10);

    db.clear_logs();
    db.synthetic_write(Durability::HIGH);

    let result = query_a(&db, input);

    assert_eq!(result.value(&db), 10);

    // What this test captures is that the interned values **must** be validated before validating their corresponding `query_x` call.
    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidValidateInternedValue { key: Interned(Id(400)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(400)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(401)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(401)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(402)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(402)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(403)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(403)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(404)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(404)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(405)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(405)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(406)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(406)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(407)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(407)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(408)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(408)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(409)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(409)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(40a)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(40a)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_a(Id(0)) })",
        ]"#]]);
}
