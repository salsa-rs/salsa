mod common;

use expect_test::expect;
use salsa::{Accumulator, Database};
use test_log::test;

#[salsa::accumulator]
struct Log(#[allow(dead_code)] String);

#[salsa::tracked]
fn push_logs(db: &dyn Database) {
    push_a_logs(db);
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Database) {
    Log("log a".to_string()).accumulate(db);
    push_b_logs(db);
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Database) {
    // No logs
    push_c_logs(db);
}

#[salsa::tracked]
fn push_c_logs(db: &dyn Database) {
    // No logs
    push_d_logs(db);
}

#[salsa::tracked]
fn push_d_logs(db: &dyn Database) {
    Log("log d".to_string()).accumulate(db);
}

#[test]
fn accumulate_chain() {
    salsa::default_database().attach(|db| {
        let logs = push_logs::accumulated::<Log>(db);
        // Check that we don't see logs from `a` appearing twice in the input.
        expect![[r#"
            [
                Log(
                    "log a",
                ),
                Log(
                    "log d",
                ),
            ]"#]]
        .assert_eq(&format!("{:#?}", logs));
    })
}
