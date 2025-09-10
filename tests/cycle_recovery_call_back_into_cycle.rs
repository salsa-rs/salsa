#![cfg(feature = "inventory")]

//! Calling back into the same cycle from your cycle recovery function _can_ work out, as long as
//! the overall cycle still converges.

mod common;
use common::{DatabaseWithValue, ValueDatabase};

#[salsa::tracked]
fn fallback_value(db: &dyn ValueDatabase) -> u32 {
    query(db) + db.get_value()
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query(db: &dyn ValueDatabase) -> u32 {
    let val = query(db);
    if val < 5 {
        val + 1
    } else {
        val
    }
}

fn cycle_initial(_db: &dyn ValueDatabase) -> u32 {
    0
}

fn cycle_fn(db: &dyn ValueDatabase, _value: &u32, _count: u32) -> salsa::CycleRecoveryAction<u32> {
    salsa::CycleRecoveryAction::Fallback(fallback_value(db))
}

#[test]
fn converges() {
    let db = DatabaseWithValue::new(10);

    assert_eq!(query(&db), 10);
}

#[test]
fn diverges() {
    let db = DatabaseWithValue::new(3);

    query(&db);
}
