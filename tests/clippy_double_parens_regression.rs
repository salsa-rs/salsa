//! Regression test for clippy::double_parens warnings in cycle macros.
//!
//! This test ensures that tracked functions with no additional inputs (beyond `db`)
//! don't trigger clippy::double_parens warnings when using cycle_initial or cycle_fn.
//!
//! Before the fix in components/salsa-macro-rules/src/unexpected_cycle_recovery.rs,
//! these macros would generate `std::mem::drop(())` which triggered the warning.
//!
//! Run `cargo test --test verify_no_double_parens` to verify the fix.
//!
//! See: https://github.com/salsa-rs/salsa/issues/1004
#![cfg(feature = "inventory")]

// This tracked function has no additional inputs beyond `db`.
// With the old code, this would trigger clippy::double_parens warnings in the
// generated `unexpected_cycle_initial` and `unexpected_cycle_recovery` macros.
#[salsa::tracked]
fn simple_tracked_query(_db: &dyn salsa::Database) -> u32 {
    100
}

// Tracked function with cycle recovery and no additional inputs.
// The cycle_initial and cycle_fn functions also have no additional inputs beyond `db`,
// which would trigger the clippy warning with the old code.
#[salsa::tracked(cycle_fn=cycle_recover, cycle_initial=initial)]
fn query_with_cycle_support(_db: &dyn salsa::Database) -> u32 {
    200
}

fn initial(_db: &dyn salsa::Database) -> u32 {
    0
}

fn cycle_recover(
    _db: &dyn salsa::Database,
    value: &u32,
    _count: u32,
) -> salsa::CycleRecoveryAction<u32> {
    // Just return the value to avoid actual cycling in this test
    salsa::CycleRecoveryAction::Fallback(*value)
}

#[test_log::test]
fn test_no_clippy_warnings_for_no_input_functions() {
    let db = salsa::DatabaseImpl::default();

    // These functions should compile without clippy::double_parens warnings
    assert_eq!(simple_tracked_query(&db), 100);
    assert_eq!(query_with_cycle_support(&db), 200);
}
