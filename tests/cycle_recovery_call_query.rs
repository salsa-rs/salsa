#![cfg(feature = "inventory")]

//! It's possible to call a Salsa query from within a cycle recovery fn.

#[salsa::tracked]
fn fallback_value(_db: &dyn salsa::Database) -> u32 {
    10
}

#[salsa::tracked(cycle_fn = |db, _, _, _| fallback_value(db), cycle_initial = |_, _| 0)]
fn query(db: &dyn salsa::Database) -> u32 {
    let val = query(db);
    if val < 5 {
        val + 1
    } else {
        val
    }
}

#[test_log::test]
fn the_test() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(query(&db), 10);
}
