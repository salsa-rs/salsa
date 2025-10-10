// Shuttle doesn't like panics inside of its runtime.
#![cfg(not(feature = "shuttle"))]

//! Tests that salsa doesn't get stuck after a panic in a nested cycle function.

use crate::sync::thread;
use crate::{Knobs, KnobsDatabase};
use std::panic::catch_unwind;

use salsa::CycleRecoveryAction;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, salsa::Update)]
struct CycleValue(u32);

const MIN: CycleValue = CycleValue(0);
const MAX: CycleValue = CycleValue(3);

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_a(db: &dyn KnobsDatabase) -> CycleValue {
    db.signal(1);
    query_b(db)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_b(db: &dyn KnobsDatabase) -> CycleValue {
    let c_value = query_c(db);
    CycleValue(c_value.0 + 1).min(MAX)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_c(db: &dyn KnobsDatabase) -> CycleValue {
    let d_value = query_d(db);

    if d_value > CycleValue(0) {
        let _e_value = query_e(db);
        let _b = query_b(db);
        db.wait_for(2);
        db.signal(3);
        panic!("Dragons are real");
    } else {
        let a_value = query_a(db);
        CycleValue(d_value.0.max(a_value.0))
    }
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_d(db: &dyn KnobsDatabase) -> CycleValue {
    query_c(db)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=initial)]
fn query_e(db: &dyn KnobsDatabase) -> CycleValue {
    query_c(db)
}

fn cycle_fn(
    _db: &dyn KnobsDatabase,
    _value: &CycleValue,
    _count: u32,
) -> CycleRecoveryAction<CycleValue> {
    CycleRecoveryAction::Iterate
}

fn initial(_db: &dyn KnobsDatabase) -> CycleValue {
    MIN
}

#[test_log::test]
fn the_test() {
    tracing::debug!("Starting new run");
    let db_t1 = Knobs::default();
    let db_t2 = db_t1.clone();
    let db_t3 = db_t1.clone();
    let db_t4 = db_t1.clone();

    let t1 = thread::spawn(move || {
        let _span = tracing::debug_span!("t1", thread_id = ?thread::current().id()).entered();

        let result = query_a(&db_t1);
        result
    });
    let t2 = thread::spawn(move || {
        let _span = tracing::debug_span!("t4", thread_id = ?thread::current().id()).entered();
        db_t4.wait_for(1);
        db_t4.signal(2);
        query_b(&db_t4)
    });
    let t3 = thread::spawn(move || {
        let _span = tracing::debug_span!("t2", thread_id = ?thread::current().id()).entered();
        db_t2.wait_for(1);
        query_d(&db_t2)
    });

    let r_t1 = t1.join();
    let r_t2 = t2.join();
    let r_t3 = t3.join();

    assert!(r_t1.is_err());
    assert!(r_t2.is_err());
    assert!(r_t3.is_err());

    // Pulling the cycle again at a later point should still result in a panic.
    assert!(catch_unwind(|| query_d(&db_t3)).is_err());
}
