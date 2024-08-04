//! Test for cycle recover spread across two threads.
//! See `../cycles.rs` for a complete listing of cycle tests,
//! both intra and cross thread.

use crate::setup::{Knobs, KnobsDatabase};

#[salsa::input]
pub(crate) struct MyInput {
    field: i32,
}

#[salsa::tracked]
pub(crate) fn a1(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(1);
    db.wait_for(2);

    a2(db, input)
}

#[salsa::tracked(recovery_fn=recover)]
pub(crate) fn a2(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    b1(db, input)
}

fn recover(db: &dyn KnobsDatabase, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover");
    key.field(db) * 20 + 2
}

#[salsa::tracked]
pub(crate) fn b1(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(1);
    db.signal(2);

    // Wait for thread A to block on this thread
    db.wait_for(3);
    b2(db, input)
}

#[salsa::tracked]
pub(crate) fn b2(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    a1(db, input)
}

// Recover cycle test:
//
// The pattern is as follows.
//
// Thread A                   Thread B
// --------                   --------
// a1                         b1
// |                          wait for stage 1 (blocks)
// signal stage 1             |
// wait for stage 2 (blocks)  (unblocked)
// |                          signal stage 2
// (unblocked)                wait for stage 3 (blocks)
// a2                         |
// b1 (blocks -> stage 3)     |
// |                          (unblocked)
// |                          b2
// |                          a1 (cycle detected)
// a2 recovery fn executes    |
// a1 completes normally      |
//                            b2 completes, recovers
//                            b1 completes, recovers

#[test]
fn execute() {
    let db = Knobs::default();

    let input = MyInput::new(&db, 1);

    let thread_a = std::thread::spawn({
        let db = db.clone();
        db.knobs().signal_on_will_block.store(3);
        move || a1(&db, input)
    });

    let thread_b = std::thread::spawn({
        let db = db.clone();
        move || b1(&db, input)
    });

    // We expect that the recovery function yields
    // `1 * 20 + 2`, which is returned (and forwarded)
    // to b1, and from there to a2 and a1.
    assert_eq!(thread_a.join().unwrap(), 22);
    assert_eq!(thread_b.join().unwrap(), 22);
}
