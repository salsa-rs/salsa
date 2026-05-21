// Shuttle doesn't like panics inside of its runtime.
#![cfg(not(feature = "shuttle"))]

use salsa::{Cancelled, Database};

use crate::setup::{Knobs, KnobsDatabase};
use crate::sync::thread;

#[salsa::tracked(cycle_initial=initial)]
fn query_a(db: &dyn KnobsDatabase) -> u32 {
    let value = query_b(db);

    db.signal(1);
    db.wait_for(2);
    cancellation_point(db);

    value
}

#[salsa::tracked(cycle_initial=initial)]
fn query_b(db: &dyn KnobsDatabase) -> u32 {
    query_a(db)
}

#[salsa::tracked]
fn cancellation_point(_db: &dyn KnobsDatabase) {}

fn initial(_db: &dyn KnobsDatabase, _id: salsa::Id) -> u32 {
    0
}

#[test]
fn cancellation_rejects_provisional_fixpoint_state() {
    let mut db = Knobs::default();
    let db_worker = db.clone();

    db.signal_on_did_cancel(2);

    let worker = thread::spawn(move || Cancelled::catch(|| query_a(&db_worker)));

    db.wait_for(1);
    db.trigger_cancellation();

    let cancelled = worker.join().unwrap();
    assert!(matches!(cancelled, Err(Cancelled::PendingWrite)));

    assert_eq!(query_a(&db), 0);
}
