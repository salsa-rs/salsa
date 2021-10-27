//! Tests for cycles that occur across threads. See the
//! `../cycles.rs` for a complete listing of cycle tests,
//! both intra and cross thread.

use crate::setup::{Knobs, ParDatabase, ParDatabaseImpl};
use salsa::{Cancelled, ParallelDatabase};
use test_env_log::test;

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

pub(crate) fn recover_from_cycle_a1(_db: &dyn ParDatabase, _cycle: &[String], key: &i32) -> i32 {
    log::debug!("recover_from_cycle_a1");
    key * 10 + 1
}

pub(crate) fn recover_from_cycle_a2(_db: &dyn ParDatabase, _cycle: &[String], key: &i32) -> i32 {
    log::debug!("recover_from_cycle_a2");
    key * 10 + 2
}

pub(crate) fn recover_from_cycle_b1(_db: &dyn ParDatabase, _cycle: &[String], key: &i32) -> i32 {
    log::debug!("recover_from_cycle_b1");
    key * 20 + 1
}

pub(crate) fn recover_from_cycle_b2(_db: &dyn ParDatabase, _cycle: &[String], key: &i32) -> i32 {
    log::debug!("recover_from_cycle_b2");
    key * 20 + 2
}

pub(crate) fn recover_cycle_a1(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(1);
    db.wait_for(2);

    db.recover_cycle_a2(key)
}

pub(crate) fn recover_cycle_a2(db: &dyn ParDatabase, key: i32) -> i32 {
    db.recover_cycle_b1(key)
}

pub(crate) fn recover_cycle_b1(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(1);
    db.signal(2);

    // Wait for thread A to block on this thread
    db.wait_for(3);

    db.recover_cycle_b2(key)
}

pub(crate) fn recover_cycle_b2(db: &dyn ParDatabase, key: i32) -> i32 {
    db.recover_cycle_a1(key)
}

pub(crate) fn panic_cycle_a(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(1);
    db.wait_for(2);

    db.panic_cycle_b(key)
}

pub(crate) fn panic_cycle_b(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(1);
    db.signal(2);

    // Wait for thread A to block on this thread
    db.wait_for(3);

    // Now try to execute A
    db.panic_cycle_a(key)
}

#[test]
fn recover_parallel_cycle() {
    let mut db = ParDatabaseImpl::default();
    db.knobs().signal_on_will_block.set(3);

    let thread_a = std::thread::spawn({
        let db = db.snapshot();
        move || db.recover_cycle_a1(1)
    });

    let thread_b = std::thread::spawn({
        let db = db.snapshot();
        move || db.recover_cycle_b1(1)
    });

    assert_eq!(thread_a.join().unwrap(), 11);
    assert_eq!(thread_b.join().unwrap(), 21);
}

#[test]
fn panic_parallel_cycle() {
    let db = ParDatabaseImpl::default();
    db.knobs().signal_on_will_block.set(3);

    let thread_a = std::thread::spawn({
        let db = db.snapshot();
        move || db.panic_cycle_a(-1)
    });

    let thread_b = std::thread::spawn({
        let db = db.snapshot();
        move || db.panic_cycle_b(-1)
    });

    // We expect B to panic because it detects a cycle (it is the one that calls A, ultimately).
    // Right now, it panics with a string.
    let err_b = thread_b.join().unwrap_err();
    if let Some(str_b) = err_b.downcast_ref::<String>() {
        assert!(
            str_b.contains("cycle detected"),
            "unexpeced string: {:?}",
            str_b
        );
    } else {
        panic!("b failed in an unexpected way");
    }

    // We expect A to propagate a panic, which causes us to use the sentinel
    // type `Canceled`.
    assert!(thread_a
        .join()
        .unwrap_err()
        .downcast_ref::<Cancelled>()
        .is_some());
}
