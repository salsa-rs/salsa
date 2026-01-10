use crate::common::{ExecuteValidateLoggerDatabase, LogDatabase};
use expect_test::expect;
use salsa::{Database, Durability};

mod common;

#[salsa::tracked(cycle_initial=a_cycle_initial)]
fn query_a(db: &dyn Database) {
    let b = query_b(db);
    query_d(db, b);
}

fn a_cycle_initial(_db: &dyn Database, _id: salsa::Id) {
    
}

#[salsa::interned]
struct Interned {
    value: u32,
}

#[salsa::tracked(cycle_initial=|db, _| Interned::new(db, 0))]
fn query_b<'db>(db: &'db dyn Database) -> Interned<'db> {
    query_c(db);
    Interned::new(db, 2)
}

#[salsa::tracked]
fn query_c(db: &dyn Database) {
    query_a(db);
}

#[salsa::tracked]
fn query_d<'db>(_db: &'db dyn Database, _i: Interned<'db>) {
    // reads some input
}

#[test_log::test]
fn the_test() {
    let mut db = ExecuteValidateLoggerDatabase::default();

    // We compute the result starting from query a...
    query_a(&db);

    db.clear_logs();
    db.synthetic_write(Durability::HIGH);

    // ...but we now verify query_b
    query_b(&db);

    // What this test captures is that `Interned(Id(c00))` must be verified **before** `query_d(Id(c00))`
    // as we would when starting from `query_a`
    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidValidateInternedValue { key: query_b::interned_arguments(Id(400)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: query_a::interned_arguments(Id(0)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: query_b::interned_arguments(Id(400)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: query_c::interned_arguments(Id(800)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: query_a::interned_arguments(Id(0)), revision: R2 })",
            "salsa_event(DidValidateInternedValue { key: Interned(Id(c00)), revision: R2 })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_d(Id(c00)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_a(Id(0)) })",
            "salsa_event(DidValidateMemoizedValue { database_key: query_b(Id(400)) })",
        ]"#]]);
}
