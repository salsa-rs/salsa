//! Test for cycle recover spread across two threads.
//! See `../cycles.rs` for a complete listing of cycle tests,
//! both intra and cross thread.

use crate::setup::Database;
use crate::setup::Knobs;
use salsa::ParallelDatabase;

pub(crate) trait Db: salsa::DbWithJar<Jar> + Knobs {}

impl<T: salsa::DbWithJar<Jar> + Knobs> Db for T {}

#[salsa::jar(db = Db)]
pub(crate) struct Jar(MyInput, a1, a2, b1, b2);

#[salsa::input(jar = Jar)]
pub(crate) struct MyInput {
    field: i32,
}

#[salsa::tracked(jar = Jar, recovery_fn=recover_a1)]
pub(crate) fn a1(db: &dyn Db, input: MyInput) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(1);
    db.wait_for(2);

    a2(db, input)
}

fn recover_a1(db: &dyn Db, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover_a1");
    key.field(db) * 10 + 1
}

#[salsa::tracked(jar = Jar, recovery_fn=recover_a2)]
pub(crate) fn a2(db: &dyn Db, input: MyInput) -> i32 {
    b1(db, input)
}

fn recover_a2(db: &dyn Db, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover_a2");
    key.field(db) * 10 + 2
}

#[salsa::tracked(jar = Jar, recovery_fn=recover_b1)]
pub(crate) fn b1(db: &dyn Db, input: MyInput) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(1);
    db.signal(2);

    // Wait for thread A to block on this thread
    db.wait_for(3);
    b2(db, input)
}

fn recover_b1(db: &dyn Db, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover_b1");
    key.field(db) * 20 + 1
}

#[salsa::tracked(jar = Jar, recovery_fn=recover_b2)]
pub(crate) fn b2(db: &dyn Db, input: MyInput) -> i32 {
    a1(db, input)
}

fn recover_b2(db: &dyn Db, _cycle: &salsa::Cycle, key: MyInput) -> i32 {
    dbg!("recover_b2");
    key.field(db) * 20 + 2
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
// |                          a1 (cycle detected, recovers)
// |                          b2 completes, recovers
// |                          b1 completes, recovers
// a2 sees cycle, recovers
// a1 completes, recovers

#[test]
fn execute() {
    let db = Database::default();
    db.knobs().signal_on_will_block.set(3);

    let input = MyInput::new(&db, 1);

    let thread_a = std::thread::spawn({
        let db = db.snapshot();
        move || a1(&*db, input)
    });

    let thread_b = std::thread::spawn({
        let db = db.snapshot();
        move || b1(&*db, input)
    });

    assert_eq!(thread_a.join().unwrap(), 11);
    assert_eq!(thread_b.join().unwrap(), 21);
}
