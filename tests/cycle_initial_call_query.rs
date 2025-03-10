//! It's possible to call a Salsa query from within a cycle initial fn.

#[salsa::tracked]
fn initial_value(_db: &dyn salsa::Database) -> u32 {
    0
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query(db: &dyn salsa::Database) -> u32 {
    let val = query(db);
    if val < 5 {
        val + 1
    } else {
        val
    }
}

fn cycle_initial(db: &dyn salsa::Database) -> u32 {
    initial_value(db)
}

fn cycle_fn(
    _db: &dyn salsa::Database,
    _value: &u32,
    _count: u32,
) -> salsa::CycleRecoveryAction<u32> {
    salsa::CycleRecoveryAction::Iterate
}

#[test_log::test]
fn the_test() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(query(&db), 5);
}
