//! Demonstrates that accumulation is done in the order
//! in which things were originally executed.

mod common;

use expect_test::expect;
use salsa::{Accumulator, Database};
use test_log::test;

#[salsa::accumulator]
struct Log(#[allow(dead_code)] String);

#[salsa::tracked]
fn push_logs(db: &dyn Database) -> salsa::Result<()> {
    push_a_logs(db)
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log a".to_string()).accumulate(db);
    push_b_logs(db)?;
    push_c_logs(db)?;
    push_d_logs(db)
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log b".to_string()).accumulate(db);
    push_d_logs(db)
}

#[salsa::tracked]
fn push_c_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log c".to_string()).accumulate(db);
    Ok(())
}

#[salsa::tracked]
fn push_d_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log d".to_string()).accumulate(db);
    Ok(())
}

#[test]
fn accumulate_execution_order() -> salsa::Result<()> {
    salsa::DatabaseImpl::new().attach(|db| {
        let logs = push_logs::accumulated::<Log>(db)?;
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
        .assert_eq(&format!("{:#?}", logs));

        Ok(())
    })
}
