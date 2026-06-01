// Shuttle doesn't like panics inside of its runtime.
#![cfg(not(feature = "shuttle"))]

//! Test for cancellation when another query is blocked on the cancelled thread.
use std::panic::catch_unwind;

use salsa::{Cancelled, Database};

use crate::setup::{Knobs, KnobsDatabase};

#[salsa::tracked]
fn query_a(db: &dyn KnobsDatabase) -> u32 {
    query_b(db)
}

#[salsa::tracked]
fn query_b(db: &dyn KnobsDatabase) -> u32 {
    db.signal(1);
    db.wait_for(3);
    query_c(db)
}

#[salsa::tracked]
fn query_c(_db: &dyn KnobsDatabase) -> u32 {
    1
}

#[salsa::tracked(cycle_initial = cycle_initial)]
fn cycle_query(db: &dyn KnobsDatabase) -> u32 {
    db.signal(1);
    db.wait_for(2);
    inner_query(db)
}

#[salsa::tracked]
fn inner_query(_db: &dyn KnobsDatabase) -> u32 {
    42
}

fn cycle_initial(_db: &dyn KnobsDatabase, _id: salsa::Id) -> u32 {
    0
}

#[test]
fn execute() {
    let db = Knobs::default();
    let db2 = db.clone();
    let db_signaler = db.clone();
    let token = db.cancellation_token();

    let t1 = std::thread::spawn(move || query_a(&db));
    db_signaler.wait_for(1);
    db2.signal_on_will_block(2);
    let t2 = std::thread::spawn(move || query_a(&db2));
    db_signaler.wait_for(2);
    token.cancel();
    db_signaler.signal(3);
    let (r1, r2) = (t1.join(), t2.join());
    let r1 = *r1.unwrap_err().downcast::<salsa::Cancelled>().unwrap();
    assert!(matches!(r1, Cancelled::Local), "{r1:?}");
    assert_eq!(r2.unwrap(), 1);
}

#[test]
fn global_cancellation_recomputes_cycle_query() {
    let db = Knobs::default();
    let db_writer = db.clone();
    let db_t1 = db.clone();
    let db_waiter = db.clone();
    db.signal_on_did_cancel(2);
    drop(db);

    let t1 = std::thread::spawn(move || catch_unwind(|| cycle_query(&db_t1)));

    db_waiter.wait_for(1);
    drop(db_waiter);

    let t2 = std::thread::spawn(move || {
        let mut db_writer = db_writer;
        db_writer.trigger_cancellation();
        db_writer
    });

    let err = *t1
        .join()
        .unwrap()
        .unwrap_err()
        .downcast::<Cancelled>()
        .unwrap();
    assert!(matches!(err, Cancelled::PendingWrite), "{err:?}");

    let db_after = t2.join().unwrap();
    assert_eq!(cycle_query(&db_after), 42);
}
