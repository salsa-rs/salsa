mod common;

use expect_test::expect;
use salsa::{Accumulator, Database};
use test_log::test;

#[salsa::input]
struct MyInput {
    count: u32,
}

#[salsa::accumulator(no_clone)]
struct Log(String);

impl Clone for Log {
    fn clone(&self) -> Self {
        Self(format!("{}.clone()", self.0))
    }
}

#[salsa::tracked]
fn push_logs(db: &dyn salsa::Database, input: MyInput) {
    for i in 0..input.count(db) {
        Log(format!("#{i}")).accumulate(db);
    }
}

#[test]
fn accumulate_custom_clone() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = MyInput::new(db, 2);
        let logs = push_logs::accumulated::<Log>(db, input);
        expect![[r##"
            [
                Log(
                    "#0.clone()",
                ),
                Log(
                    "#1.clone()",
                ),
            ]
        "##]]
        .assert_debug_eq(&logs);
    })
}
