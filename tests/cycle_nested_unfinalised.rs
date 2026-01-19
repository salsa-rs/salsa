//! Test demonstrating that nested cycle participants may not be finalized.
//!
//! The issue is:
//! 1. `query_a` is the outer cycle head
//! 2. `query_c` is a nested cycle head that depends on `query_d`
//! 3. `query_d` depends back on `query_c` (forming a nested cycle)
//! 4. When `query_c` completes as a nested cycle, it flattens its dependencies
//! 5. `query_d` gets inlined (removed from dependency tree) because it's provisional
//! 6. When `query_a` finalizes, `query_d` is not reachable and never gets finalized
//! 7. Later queries reading `query_d` see it as provisional and have to re-execute
//!
//! Call graph:
//! * `query_a` -> `query_b` -> `query_c` -> `query_d` -> `query_c` (cycle)
//!                                       -> `query_a` (cycle back to outer)
//!
//! After `query_a` converges, calling `query_d` separately should return the
//! converged value, not the initial value.

#![cfg(feature = "inventory")]

use crate::common::{ExecuteValidateLoggerDatabase, LogDatabase};
use expect_test::expect;

mod common;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, salsa::Update)]
struct CycleValue(u32);

const MIN: CycleValue = CycleValue(0);
const MAX: CycleValue = CycleValue(3);

/// Outer cycle head: a -> b
#[salsa::tracked(cycle_initial=initial)]
fn query_a(db: &dyn salsa::Database) -> CycleValue {
    query_b(db)
}

/// b -> c, increments value
#[salsa::tracked(cycle_initial=initial)]
fn query_b(db: &dyn salsa::Database) -> CycleValue {
    let c_value = query_c(db);
    CycleValue(c_value.0 + 1).min(MAX)
}

/// Nested cycle head: c -> d, then back to a
#[salsa::tracked(cycle_initial=initial)]
fn query_c(db: &dyn salsa::Database) -> CycleValue {
    let d_value = query_d(db);
    let a_value = query_a(db);
    CycleValue(d_value.0.max(a_value.0))
}

/// d -> c (completes the nested cycle)
#[salsa::tracked(cycle_initial=initial)]
fn query_d(db: &dyn salsa::Database) -> CycleValue {
    query_c(db)
}

fn initial(_db: &dyn salsa::Database, _id: salsa::Id) -> CycleValue {
    MIN
}

#[test_log::test]
fn nested_cycle_participant_is_finalized() {
    let db = ExecuteValidateLoggerDatabase::default();

    // First, run query_a which drives the entire cycle to convergence.
    let a_result = query_a(&db);
    assert_eq!(a_result, MAX);

    // Note: query_d is only executed once in the first iteration, then gets
    // flattened out of the dependency tree and is not re-executed during
    // subsequent iterations.
    db.assert_logs(expect![[r#"
        [
            "salsa_event(WillExecute { database_key: query_a(Id(0)) })",
            "salsa_event(WillExecute { database_key: query_b(Id(400)) })",
            "salsa_event(WillExecute { database_key: query_c(Id(800)) })",
            "salsa_event(WillExecute { database_key: query_d(Id(c00)) })",
            "salsa_event(WillIterateCycle { database_key: query_a(Id(0)), iteration_count: IterationCount(1) })",
            "salsa_event(WillExecute { database_key: query_b(Id(400)) })",
            "salsa_event(WillExecute { database_key: query_c(Id(800)) })",
            "salsa_event(WillExecute { database_key: query_d(Id(c00)) })",
            "salsa_event(WillIterateCycle { database_key: query_a(Id(0)), iteration_count: IterationCount(2) })",
            "salsa_event(WillExecute { database_key: query_b(Id(400)) })",
            "salsa_event(WillExecute { database_key: query_c(Id(800)) })",
            "salsa_event(WillExecute { database_key: query_d(Id(c00)) })",
            "salsa_event(WillIterateCycle { database_key: query_a(Id(0)), iteration_count: IterationCount(3) })",
            "salsa_event(WillExecute { database_key: query_b(Id(400)) })",
            "salsa_event(WillExecute { database_key: query_c(Id(800)) })",
            "salsa_event(WillExecute { database_key: query_d(Id(c00)) })",
            "salsa_event(WillIterateCycle { database_key: query_a(Id(0)), iteration_count: IterationCount(4) })",
            "salsa_event(WillExecute { database_key: query_b(Id(400)) })",
            "salsa_event(WillExecute { database_key: query_c(Id(800)) })",
            "salsa_event(WillExecute { database_key: query_d(Id(c00)) })",
            "salsa_event(DidFinalizeCycle { database_key: query_a(Id(0)), iteration_count: IterationCount(4) })",
        ]"#]]);

    // Now query_d separately. It should return the converged value (MAX),
    // not the initial value (MIN).
    let d_result = query_d(&db);
    assert_eq!(
        d_result, MAX,
        "query_d should return converged value after query_a completes"
    );

    // BUG: query_d was not finalized when query_a's cycle completed, so it
    // gets re-executed here. This is inefficient but at least produces the
    // correct result. In the multi-threaded case, this can cause incorrect
    // results if another thread reads the stale provisional value before
    // re-execution completes.
    //
    // The expected behavior (once fixed) would be an empty log here, indicating
    // that query_d was properly memoized from the cycle.
    db.assert_logs(expect!["[]"]);
}
