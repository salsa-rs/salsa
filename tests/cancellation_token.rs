#![cfg(feature = "inventory")]
//! Test that `DeriveWithDb` is correctly derived.

mod common;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Barrier,
    thread,
    time::Duration,
};

use crossbeam_channel::{RecvTimeoutError, TryRecvError};
use expect_test::expect;
use salsa::{Cancelled, Database, DatabaseImpl};

use crate::common::LogDatabase;

#[salsa::input(debug)]
struct MyInput {
    #[returns(copy)]
    field: u32,
}

#[salsa::tracked(returns(copy))]
fn a(db: &dyn Database, input: MyInput) -> u32 {
    BARRIER.wait();
    BARRIER2.wait();
    b(db, input)
}
#[salsa::tracked(returns(copy))]
fn b(db: &dyn Database, input: MyInput) -> u32 {
    input.field(db)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn panicking_cycle_query(_db: &dyn Database) -> u32 {
    panic!("boom")
}

#[salsa::tracked(returns(copy))]
fn uncancelled_query(_db: &dyn Database) -> u32 {
    1
}

#[salsa::tracked(returns(copy))]
fn cancel_after_cycle_panic(db: &dyn Database) -> u32 {
    assert!(catch_unwind(AssertUnwindSafe(|| panicking_cycle_query(db))).is_err());
    db.cancellation_token().cancel();
    uncancelled_query(db)
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
    assert!(matches!(res, Err(Cancelled::Local)), "{res:?}");
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

#[test]
fn cancellation_is_restored_after_cycle_panic() {
    let db = common::LoggerDatabase::default();
    let result = Cancelled::catch(|| cancel_after_cycle_panic(&db));
    assert!(matches!(result, Err(Cancelled::Local)), "{result:?}");
}

#[test]
fn cancellation_receiver_disconnects_when_revision_is_cancelled() {
    let mut db = DatabaseImpl::default();
    let receiver = db.cancellation_receiver();

    assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
    db.trigger_cancellation();
    assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn receivers_for_same_revision_share_channel() {
    let db = DatabaseImpl::default();
    let first = db.cancellation_receiver();
    let second = db.cancellation_receiver();

    assert!(first.same_channel(&second));
}

#[test]
fn later_revision_gets_fresh_cancellation_receiver() {
    let mut db = DatabaseImpl::default();
    let first = db.cancellation_receiver();

    db.trigger_cancellation();

    let second = db.cancellation_receiver();
    assert!(!first.same_channel(&second));
    assert_eq!(second.try_recv(), Err(TryRecvError::Empty));

    db.trigger_cancellation();
    assert_eq!(second.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn local_cancellation_does_not_disconnect_revision_receiver() {
    let db = DatabaseImpl::default();
    let receiver = db.cancellation_receiver();

    db.cancellation_token().cancel();

    assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn receiver_disconnects_before_pending_write_waits_for_snapshots() {
    let mut db = DatabaseImpl::default();
    let snapshot = db.clone();
    let receiver = snapshot.cancellation_receiver();

    let writer = thread::spawn(move || db.trigger_cancellation());

    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)),
        Err(RecvTimeoutError::Disconnected)
    );

    let cancelled = Cancelled::catch(|| snapshot.unwind_if_revision_cancelled());
    assert!(matches!(cancelled, Err(Cancelled::PendingWrite)));

    drop(snapshot);
    writer.join().unwrap();
}

#[test]
fn receiver_created_after_revision_cancellation_is_disconnected() {
    let mut db = DatabaseImpl::default();
    let snapshot = db.clone();
    let pending = snapshot.cancellation_receiver();

    let writer = thread::spawn(move || db.trigger_cancellation());
    assert_eq!(
        pending.recv_timeout(Duration::from_secs(5)),
        Err(RecvTimeoutError::Disconnected)
    );

    let receiver = snapshot.cancellation_receiver();
    assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
    drop(snapshot);
    writer.join().unwrap();
}
