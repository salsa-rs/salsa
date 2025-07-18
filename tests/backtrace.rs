#![cfg(feature = "inventory")]

use expect_test::expect;
use salsa::{Backtrace, Database, DatabaseImpl};
use test_log::test;

#[salsa::input(debug)]
struct Thing {
    detailed: bool,
}

#[salsa::tracked]
fn query_a(db: &dyn Database, thing: Thing) -> String {
    query_b(db, thing)
}

#[salsa::tracked]
fn query_b(db: &dyn Database, thing: Thing) -> String {
    query_c(db, thing)
}

#[salsa::tracked]
fn query_c(db: &dyn Database, thing: Thing) -> String {
    query_d(db, thing)
}

#[salsa::tracked]
fn query_d(db: &dyn Database, thing: Thing) -> String {
    query_e(db, thing)
}

#[salsa::tracked]
fn query_e(db: &dyn Database, thing: Thing) -> String {
    if thing.detailed(db) {
        format!("{:#}", Backtrace::capture().unwrap())
    } else {
        format!("{}", Backtrace::capture().unwrap())
    }
}
#[salsa::tracked]
fn query_f(db: &dyn Database, thing: Thing) -> String {
    query_cycle(db, thing)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query_cycle(db: &dyn Database, thing: Thing) -> String {
    let backtrace = query_cycle(db, thing);
    if backtrace.is_empty() {
        query_e(db, thing)
    } else {
        backtrace
    }
}

fn cycle_initial(_db: &dyn salsa::Database, _thing: Thing) -> String {
    String::new()
}

fn cycle_fn(
    _db: &dyn salsa::Database,
    _value: &str,
    _count: u32,
    _thing: Thing,
) -> salsa::CycleRecoveryAction<String> {
    salsa::CycleRecoveryAction::Iterate
}

#[test]
fn backtrace_works() {
    let db = DatabaseImpl::default();

    let backtrace = query_a(&db, Thing::new(&db, false)).replace("\\", "/");
    expect![[r#"
        query stacktrace:
           0: query_e(Id(0))
                     at tests/backtrace.rs:32
           1: query_d(Id(0))
                     at tests/backtrace.rs:27
           2: query_c(Id(0))
                     at tests/backtrace.rs:22
           3: query_b(Id(0))
                     at tests/backtrace.rs:17
           4: query_a(Id(0))
                     at tests/backtrace.rs:12
    "#]]
    .assert_eq(&backtrace);

    let backtrace = query_a(&db, Thing::new(&db, true)).replace("\\", "/");
    expect![[r#"
        query stacktrace:
           0: query_e(Id(1)) -> (R1, Durability::LOW)
                     at tests/backtrace.rs:32
           1: query_d(Id(1)) -> (R1, Durability::HIGH)
                     at tests/backtrace.rs:27
           2: query_c(Id(1)) -> (R1, Durability::HIGH)
                     at tests/backtrace.rs:22
           3: query_b(Id(1)) -> (R1, Durability::HIGH)
                     at tests/backtrace.rs:17
           4: query_a(Id(1)) -> (R1, Durability::HIGH)
                     at tests/backtrace.rs:12
    "#]]
    .assert_eq(&backtrace);

    let backtrace = query_f(&db, Thing::new(&db, false)).replace("\\", "/");
    expect![[r#"
        query stacktrace:
           0: query_e(Id(2))
                     at tests/backtrace.rs:32
           1: query_cycle(Id(2))
                     at tests/backtrace.rs:45
                     cycle heads: query_cycle(Id(2)) -> IterationCount(0)
           2: query_f(Id(2))
                     at tests/backtrace.rs:40
    "#]]
    .assert_eq(&backtrace);

    let backtrace = query_f(&db, Thing::new(&db, true)).replace("\\", "/");
    expect![[r#"
        query stacktrace:
           0: query_e(Id(3)) -> (R1, Durability::LOW)
                     at tests/backtrace.rs:32
           1: query_cycle(Id(3)) -> (R1, Durability::HIGH, iteration = IterationCount(0))
                     at tests/backtrace.rs:45
                     cycle heads: query_cycle(Id(3)) -> IterationCount(0)
           2: query_f(Id(3)) -> (R1, Durability::HIGH)
                     at tests/backtrace.rs:40
    "#]]
    .assert_eq(&backtrace);
}
