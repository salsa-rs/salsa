#![cfg(feature = "inventory")]

//! It is possible to omit the `cycle_fn`, only specifying `cycle_result` in which case
//! an immediate fallback value is used as the cycle handling opposed to doing a fixpoint resolution.

use std::sync::atomic::{AtomicI32, Ordering};

#[salsa::tracked(cycle_result=cycle_result)]
fn one_o_one(db: &dyn salsa::Database) -> u32 {
    let val = one_o_one(db);
    val + 1
}

fn cycle_result(_db: &dyn salsa::Database) -> u32 {
    100
}

#[test_log::test]
fn simple() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(one_o_one(&db), 100);
}

#[salsa::tracked(cycle_result=two_queries_cycle_result)]
fn two_queries1(db: &dyn salsa::Database) -> i32 {
    two_queries2(db);
    0
}

#[salsa::tracked]
fn two_queries2(db: &dyn salsa::Database) -> i32 {
    two_queries1(db);
    // This is horribly against Salsa's rules, but we want to test that
    // the value from within the cycle is not considered, and this is
    // the only way I found.
    static CALLS_COUNT: AtomicI32 = AtomicI32::new(0);
    CALLS_COUNT.fetch_add(1, Ordering::Relaxed)
}

fn two_queries_cycle_result(_db: &dyn salsa::Database) -> i32 {
    1
}

#[test]
fn two_queries() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(two_queries1(&db), 1);
    assert_eq!(two_queries2(&db), 1);
}
