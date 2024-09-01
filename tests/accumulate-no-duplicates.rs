//! Test that we don't get duplicate accumulated values

mod common;

use expect_test::expect;
use salsa::{Accumulator, Database};
use test_log::test;

// A(1) {
//   B
//   B
//   C {
//     D {
//       A(2) {
//         B
//       }
//       B
//     }
//     E
//   }
//   B
// }

#[salsa::accumulator]
struct Log(#[allow(dead_code)] String);

#[salsa::input]
struct MyInput {
    n: u32,
}

#[salsa::tracked]
fn push_logs(db: &dyn Database) -> salsa::Result<()> {
    push_a_logs(db, MyInput::new(db, 1))
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Database, input: MyInput) -> salsa::Result<()> {
    Log("log a".to_string()).accumulate(db);
    if input.n(db)? == 1 {
        push_b_logs(db)?;
        push_b_logs(db)?;
        push_c_logs(db)?;
        push_b_logs(db)
    } else {
        push_b_logs(db)
    }
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log b".to_string()).accumulate(db);
    Ok(())
}

#[salsa::tracked]
fn push_c_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log c".to_string()).accumulate(db);
    push_d_logs(db)?;
    push_e_logs(db)
}

// Note this isn't tracked
fn push_d_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log d".to_string()).accumulate(db);
    push_a_logs(db, MyInput::new(db, 2))?;
    push_b_logs(db)
}

#[salsa::tracked]
fn push_e_logs(db: &dyn Database) -> salsa::Result<()> {
    Log("log e".to_string()).accumulate(db);
    Ok(())
}

#[test]
fn accumulate_no_duplicates() -> salsa::Result<()> {
    salsa::DatabaseImpl::new().attach(|db| {
        let logs = push_logs::accumulated::<Log>(db)?;
        // Test that there aren't duplicate B logs.
        // Note that log A appears twice, because they both come
        // from different inputs.
        expect![[r#"
            [
                Log(
                    "log a",
                ),
                Log(
                    "log b",
                ),
                Log(
                    "log c",
                ),
                Log(
                    "log d",
                ),
                Log(
                    "log a",
                ),
                Log(
                    "log e",
                ),
            ]"#]]
        .assert_eq(&format!("{:#?}", logs));

        Ok(())
    })
}
