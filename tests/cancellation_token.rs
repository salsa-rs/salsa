#![cfg(feature = "inventory")]
//! Test that `DeriveWithDb` is correctly derived.

mod common;

use std::{sync::Barrier, thread};

use expect_test::expect;
use salsa::{Cancelled, Database};

use crate::common::LogDatabase;

#[salsa::input(debug)]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn a(db: &dyn Database, input: MyInput) -> u32 {
    BARRIER.wait();
    BARRIER2.wait();
    b(db, input)
}
#[salsa::tracked]
fn b(db: &dyn Database, input: MyInput) -> u32 {
    input.field(db)
}

static BARRIER: Barrier = Barrier::new(2);
static BARRIER2: Barrier = Barrier::new(2);

#[test]
fn cancellation_token() {
    let db = common::EventLoggerDatabase::default();
    let token = db.cancellation_token();
    let input = MyInput::new(&db, 22);
    let res = Cancelled::catch(|| {
        thread::scope(|s| {
            s.spawn(|| {
                BARRIER.wait();
                token.cancel();
                BARRIER2.wait();
            });
            a(&db, input)
        })
    });
    assert!(matches!(res, Err(Cancelled::Cancelled)), "{res:?}");
    drop(res);
    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: a(Id(0)) }",
            "WillCheckCancellation",
        ]"#]]);
    thread::spawn(|| {
        BARRIER.wait();
        BARRIER2.wait();
    });
    a(&db, input);
    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: a(Id(0)) }",
            "WillCheckCancellation",
            "WillExecute { database_key: b(Id(0)) }",
        ]"#]]);
}
