use crate::common::{ExecuteValidateLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{Database, Durability, Id};

mod common;

#[salsa::tracked(cycle_initial=cycle_initial)]
fn query_a<'db>(db: &'db dyn salsa::Database) -> Interned<'db> {
    let interned = query_b(db);
    let value = interned.value(db);

    if value < 10 {
        Interned::new(db, value + 1)
    } else {
        interned
    }
}

#[salsa::tracked]
fn query_b<'db>(db: &'db dyn Database) -> Interned<'db> {
    let interned = query_a(db);
    query_x(db, interned);
    interned
}

#[salsa::tracked]
fn query_x<'db>(_db: &'db dyn Database, _i: Interned<'db>) {}

fn cycle_initial(db: &dyn Database, _id: Id) -> Interned<'_> {
    Interned::new(db, 0)
}

#[salsa::interned]
struct Interned {
    value: u32,
}

#[test_log::test]
fn the_test() {
    let mut db = ExecuteValidateLoggerDatabase::default();

    let result = query_a(&db);

    assert_eq!(result.value(&db), 10);

    db.clear_logs();
    db.synthetic_write(Durability::HIGH);

    let result = query_a(&db);

    assert_eq!(result.value(&db), 10);

    // What this test captures is that the interned values **must** be validated before validating their corresponding `query_x` call.
    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidValidateInternedValue { key: query_b::interned_arguments(Id(400)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: query_a::interned_arguments(Id(0)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(800)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(800)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(801)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(801)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(802)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(802)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(803)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(803)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(804)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(804)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(805)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(805)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(806)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(806)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(807)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(807)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(808)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(808)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(809)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(809)) })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(80a)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_x(Id(80a)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_a(Id(0)) })",
        ]"#]]);
}
