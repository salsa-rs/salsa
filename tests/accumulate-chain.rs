#![cfg(all(feature = "inventory", feature = "accumulator"))]

//! Test that when having nested tracked functions
//! we don't drop any values when accumulating.

mod common;

use expect_test::expect;
use salsa::{Accumulator, Database, DatabaseImpl};
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
    _ = input.dummy(db);
    push_a_logs(db, input);
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    Log("log a".to_string()).accumulate(db);
    push_b_logs(db, input);
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    // No logs
    push_c_logs(db, input);
}

#[salsa::tracked]
fn push_c_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    // No logs
    push_d_logs(db, input);
}

#[salsa::tracked]
fn push_d_logs(db: &dyn Database, input: Input) {
    _ = input.dummy(db);
    Log("log d".to_string()).accumulate(db);
}

#[test]
fn accumulate_chain() {
    DatabaseImpl::new().attach(|db| {
        let logs = push_logs::accumulated::<Log>(db, Input::new(db, ()));
        // Check that we get all the logs.
        expect![[r#"
            [
                Log(
                    "log a",
                ),
                Log(
                    "log d",
                ),
            ]"#]]
        .assert_eq(&format!("{logs:#?}"));
    })
}
