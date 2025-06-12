#![cfg(all(feature = "inventory", feature = "accumulator"))]

//! Demonstrates that accumulation is done in the order
//! in which things were originally executed.

mod common;

use expect_test::expect;
use salsa::{Accumulator, Database};
use test_log::test;

#[salsa::accumulator]
#[derive(Debug)]
struct Log(#[allow(dead_code)] String);

// We need a dummy input, otherwise our query will have a `NEVER_CHANGE` durability,
// and queries with this durability don't accumulate.
#[salsa::input]
struct Input {
    dummy: (),
}

#[salsa::tracked]
fn push_logs(db: &dyn Database, input: Input) {
    // We need a dummy input, otherwise our query will have a `NEVER_CHANGE` durability,
    // and queries with this durability don't accumulate.
    _ = input.dummy(db);
    push_a_logs(db, input);
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    Log("log a".to_string()).accumulate(db);
    push_b_logs(db, input);
    push_c_logs(db, input);
    push_d_logs(db, input);
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    Log("log b".to_string()).accumulate(db);
    push_d_logs(db, input);
}

#[salsa::tracked]
fn push_c_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    Log("log c".to_string()).accumulate(db);
}

#[salsa::tracked]
fn push_d_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    Log("log d".to_string()).accumulate(db);
}

#[test]
fn accumulate_execution_order() {
    salsa::DatabaseImpl::new().attach(|db| {
        let logs = push_logs::accumulated::<Log>(db, Input::new(db, ()));
        // Check that we get logs in execution order
        expect![[r#"
            [
                Log(
                    "log a",
                ),
                Log(
                    "log b",
                ),
                Log(
                    "log d",
                ),
                Log(
                    "log c",
                ),
            ]"#]]
        .assert_eq(&format!("{logs:#?}"));
    })
}
