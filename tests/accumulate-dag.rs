mod common;

use expect_test::expect;
use salsa::{Accumulator, Database};
use test_log::test;

#[salsa::input]
struct MyInput {
    field_a: u32,
    field_b: u32,
}

#[salsa::accumulator]
struct Log(#[allow(dead_code)] String);

#[salsa::tracked]
fn push_logs(db: &dyn Database, input: MyInput) {
    push_a_logs(db, input);
    push_b_logs(db, input);
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Database, input: MyInput) {
    let count = input.field_a(db);
    for i in 0..count {
        Log(format!("log_a({} of {})", i, count)).accumulate(db);
    }
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Database, input: MyInput) {
    // Note that b calls a
    push_a_logs(db, input);
    let count = input.field_b(db);
    for i in 0..count {
        Log(format!("log_b({} of {})", i, count)).accumulate(db);
    }
}

#[test]
fn accumulate_a_called_twice() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = MyInput::new(db, 2, 3);
        let logs = push_logs::accumulated::<Log>(db, input);
        // Check that we don't see logs from `a` appearing twice in the input.
        expect![[r#"
            [
                Log(
                    "log_a(0 of 2)",
                ),
                Log(
                    "log_a(1 of 2)",
                ),
                Log(
                    "log_b(0 of 3)",
                ),
                Log(
                    "log_b(1 of 3)",
                ),
                Log(
                    "log_b(2 of 3)",
                ),
            ]"#]]
        .assert_eq(&format!("{:#?}", logs));
    })
}
