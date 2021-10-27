//! Tests for cycles that occur across threads. See the
//! `../cycles.rs` for a complete listing of cycle tests,
//! both intra and cross thread.

use crate::setup::{Knobs, ParDatabase, ParDatabaseImpl};
use salsa::{Cancelled, ParallelDatabase};
use test_env_log::test;

pub(crate) fn recover_cycle(_db: &dyn ParDatabase, _cycle: &[String], key: &i32) -> i32 {
    key * 10
}

pub(crate) fn recover_cycle_a(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(0);
    db.wait_for(1);

    db.recover_cycle_b(key)
}

pub(crate) fn recover_cycle_b(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(0);
    db.signal(1);

    if key < 0 && db.should_cycle() {
        db.recover_cycle_a(key)
    } else {
        key
    }
}

pub(crate) fn panic_cycle_a(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(0);
    db.wait_for(1);

    db.panic_cycle_b(key)
}

pub(crate) fn panic_cycle_b(db: &dyn ParDatabase, key: i32) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(0);
    db.signal(1);

    // Wait for thread A to block on this thread
    db.wait_for(2);

    // Now try to execute A
    if key < 0 && db.should_cycle() {
        db.panic_cycle_a(key)
    } else {
        key
    }
}

#[test]
fn recover_parallel_cycle() {
    let mut db = ParDatabaseImpl::default();
    db.set_should_cycle(true);

    let thread_a = std::thread::spawn({
        let db = db.snapshot();
        move || db.recover_cycle_a(-1)
    });

    let thread_b = std::thread::spawn({
        let db = db.snapshot();
        move || db.recover_cycle_b(-1)
    });

    assert_eq!(thread_a.join().unwrap(), -10);
    assert_eq!(thread_b.join().unwrap(), -10);
}

#[test]
fn panic_parallel_cycle() {
    let mut db = ParDatabaseImpl::default();
    db.set_should_cycle(true);
    db.knobs().signal_on_will_block.set(2);

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
