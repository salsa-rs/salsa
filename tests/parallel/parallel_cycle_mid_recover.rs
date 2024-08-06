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
    // tell thread b we have started
    db.signal(1);

    // wait for thread b to block on a1
    db.wait_for(2);

    a2(db, input)
}

#[salsa::tracked]
pub(crate) fn a2(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    // create the cycle
    b1(db, input)
}

#[salsa::tracked(recovery_fn=recover_b1)]
pub(crate) fn b1(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    // wait for thread a to have started
    db.wait_for(1);
    b2(db, input)
}

fn recover_b1(db: &dyn KnobsDatabase, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover_b1");
    key.field(db) * 20 + 2
}

#[salsa::tracked]
pub(crate) fn b2(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    // will encounter a cycle but recover
    b3(db, input);
    b1(db, input); // hasn't recovered yet
    0
}

#[salsa::tracked(recovery_fn=recover_b3)]
pub(crate) fn b3(db: &dyn KnobsDatabase, input: MyInput) -> i32 {
    // will block on thread a, signaling stage 2
    a1(db, input)
}

fn recover_b3(db: &dyn KnobsDatabase, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover_b3");
    key.field(db) * 200 + 2
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
// |                          |
// |                          b2
// |                          b3
// |                          a1 (blocks -> stage 2)
// (unblocked)                |
// a2 (cycle detected)        |
//                            b3 recovers
//                            b2 resumes
//                            b1 recovers

#[test]
fn execute() {
    let db = Knobs::default();

    let input = MyInput::new(&db, 1);

    let thread_a = std::thread::spawn({
        let db = db.clone();
        move || a1(&db, input)
    });

    let thread_b = std::thread::spawn({
        let db = db.clone();
        db.knobs().signal_on_will_block.store(3);
        move || b1(&db, input)
    });

    // We expect that the recovery function yields
    // `1 * 20 + 2`, which is returned (and forwarded)
    // to b1, and from there to a2 and a1.
    assert_eq!(thread_a.join().unwrap(), 22);
    assert_eq!(thread_b.join().unwrap(), 22);
}
