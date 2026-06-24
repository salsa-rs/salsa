#![cfg(all(feature = "inventory", feature = "accumulator"))]

use crate::common::{ExecuteValidateLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{Database, Durability, Id};

mod common;

#[salsa::tracked(cycle_initial=cycle_initial)]
fn query_a(db: &dyn salsa::Database) -> Interned<'_> {
    let interned = query_b(db);
    let value = interned.value(db);

    if value < 10 {
        Interned::new(db, value + 1)
    } else {
        interned
    }
}

#[salsa::tracked]
fn query_b(db: &dyn Database) -> Interned<'_> {
    let interned = query_a(db);
    query_x(db, interned);
    interned
}

#[salsa::tracked]
fn query_x<'db>(db: &'db dyn Database, _i: Interned<'db>) {
    StableInput::get(db).value(db);
}

fn cycle_initial(db: &dyn Database, _id: Id) -> Interned<'_> {
    // Keep cycle-created values reusable so the test still covers validation ordering.
    db.report_untracked_read();
    Interned::new(db, 0)
}

#[salsa::interned]
struct Interned {
    value: u32,
}

#[salsa::input(singleton)]
struct StableInput {
    value: (),
}

#[test_log::test]
fn the_test() {
    let mut db = ExecuteValidateLoggerDatabase::default();
    let _ = StableInput::builder(())
        .durability(Durability::HIGH)
        .new(&db);

    let result = query_a(&db);

    assert_eq!(result.value(&db), 10);

    db.clear_logs();
    db.synthetic_write(Durability::HIGH);

    let result = query_a(&db);

    assert_eq!(result.value(&db), 10);

    // What this test captures is that the interned values **must** be validated before validating their corresponding `query_x` call.
    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c00)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c00)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c01)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c01)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c02)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c02)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c03)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c03)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c04)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c04)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c05)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c05)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c06)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c06)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c07)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c07)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c08)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c08)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c09)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c09)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c0a)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(c0a)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_a(Id(400)) })",
        ]"#]]);
}
