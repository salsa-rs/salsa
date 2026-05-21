// Shuttle doesn't like panics inside of its runtime.
#![cfg(not(feature = "shuttle"))]

use std::panic::catch_unwind;

use salsa::{Cancelled, Database};

use crate::setup::{Knobs, KnobsDatabase};
use crate::sync::thread;

#[salsa::input]
struct Input {
    value: u32,
}

#[salsa::tracked(cycle_fn = cycle_fn, cycle_initial = cycle_initial)]
fn cycle_a(db: &dyn KnobsDatabase, input: Input) -> u32 {
    let value = cycle_b(db, input);

    db.signal(1);
    db.wait_for(2);
    cancellation_point(db, input);

    value
}

#[salsa::tracked(cycle_fn = cycle_fn, cycle_initial = cycle_initial)]
fn cycle_b(db: &dyn KnobsDatabase, input: Input) -> u32 {
    let value = cycle_a(db, input);
    value.saturating_add(1).min(input.value(db))
}

#[salsa::tracked]
fn cancellation_point(db: &dyn KnobsDatabase, input: Input) {
    input.value(db);
}

fn cycle_initial(_db: &dyn KnobsDatabase, _id: salsa::Id, _input: Input) -> u32 {
    0
}

fn cycle_fn(
    _db: &dyn KnobsDatabase,
    _cycle: &salsa::Cycle,
    _last_provisional_value: &u32,
    value: u32,
    _input: Input,
) -> u32 {
    value
}

#[test]
fn pending_write_cancellation_invalidates_provisional_memos() {
    let db = Knobs::default();
    let db_writer = db.clone();
    let db_t1 = db.clone();
    let db_waiter = db.clone();
    let input = Input::new(&db, 3);

    db.signal_on_did_cancel(2);

    drop(db);

    let t1 = thread::spawn(move || catch_unwind(|| cycle_a(&db_t1, input)));

    db_waiter.wait_for(1);
    drop(db_waiter);

    let t2 = thread::spawn({
        let mut db_writer = db_writer;
        move || {
            db_writer.trigger_lru_eviction();
            db_writer
        }
    });

    let result = t1.join().unwrap();
    let Err(payload) = result else {
        panic!("expected the fixpoint query to be cancelled");
    };
    assert!(
        payload
            .downcast_ref::<Cancelled>()
            .is_some_and(|cancelled| matches!(cancelled, Cancelled::PendingWrite)),
        "expected pending-write cancellation, got {payload:?}",
    );

    let db_after = t2.join().unwrap();

    let result = catch_unwind(|| cycle_a(&db_after, input));
    assert_eq!(result.unwrap(), 3);
}
