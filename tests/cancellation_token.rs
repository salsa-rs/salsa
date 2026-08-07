#![cfg(feature = "inventory")]
//! Test that `DeriveWithDb` is correctly derived.

mod common;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

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
fn callback_runs_every_time_revision_is_cancelled() {
    let mut db = DatabaseImpl::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let registration = db.on_cancellation(Box::new({
        let calls = Arc::clone(&calls);
        move || {
            calls.fetch_add(1, Ordering::Relaxed);
        }
    }));

    db.trigger_cancellation();
    db.trigger_cancellation();

    assert_eq!(calls.load(Ordering::Relaxed), 2);

    registration.unregister();
}

#[test]
fn local_cancellation_does_not_notify_revision_callbacks() {
    let db = DatabaseImpl::default();
    let called = Arc::new(AtomicBool::new(false));
    let registration = db.on_cancellation(Box::new({
        let called = Arc::clone(&called);
        move || called.store(true, Ordering::Relaxed)
    }));

    db.cancellation_token().cancel();

    assert!(!called.load(Ordering::Relaxed));

    registration.unregister();
}

#[test]
fn callback_runs_before_pending_write_waits_for_snapshots() {
    let mut db = DatabaseImpl::default();
    let snapshot = db.clone();
    let (sender, receiver) = mpsc::channel();
    let registration = snapshot.on_cancellation(Box::new(move || {
        sender.send(()).unwrap();
    }));

    let writer = thread::spawn(move || db.trigger_cancellation());

    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("cancellation callback must run before the writer waits for snapshots");

    let cancelled = Cancelled::catch(|| snapshot.unwind_if_revision_cancelled());
    assert!(matches!(cancelled, Err(Cancelled::PendingWrite)));

    registration.unregister();
    drop(snapshot);

    writer.join().unwrap();
}

#[test]
fn unregistering_prevents_callback() {
    let mut db = DatabaseImpl::default();
    let called = Arc::new(AtomicBool::new(false));
    let registration = db.on_cancellation(Box::new({
        let called = Arc::clone(&called);
        move || called.store(true, Ordering::Relaxed)
    }));

    registration.unregister();
    db.trigger_cancellation();

    assert!(!called.load(Ordering::Relaxed));
}

#[test]
fn dropping_registration_does_not_unregister_callback() {
    let mut db = DatabaseImpl::default();
    let called = Arc::new(AtomicBool::new(false));
    let registration = db.on_cancellation(Box::new({
        let called = Arc::clone(&called);
        move || called.store(true, Ordering::Relaxed)
    }));

    drop(registration);
    db.trigger_cancellation();

    assert!(called.load(Ordering::Relaxed));
}

#[test]
fn callback_registered_after_revision_cancellation_runs_immediately() {
    let mut db = DatabaseImpl::default();
    let snapshot = db.clone();
    let (sender, receiver) = mpsc::channel();
    let pending_registration = snapshot.on_cancellation(Box::new(move || {
        sender.send(()).unwrap();
    }));

    let writer = thread::spawn(move || db.trigger_cancellation());
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("cancellation callback must run before the writer waits for snapshots");

    let called = Arc::new(AtomicBool::new(false));
    let registration = snapshot.on_cancellation(Box::new({
        let called = Arc::clone(&called);
        move || called.store(true, Ordering::Relaxed)
    }));

    assert!(called.load(Ordering::Relaxed));

    registration.unregister();
    pending_registration.unregister();
    drop(snapshot);
    writer.join().unwrap();
}

#[test]
fn cancellation_callback_panic_does_not_leave_database_cancelled() {
    let mut db = DatabaseImpl::default();
    let registration = db.on_cancellation(Box::new(|| panic!("cancellation callback panic")));

    let result = catch_unwind(AssertUnwindSafe(|| db.trigger_cancellation()));
    assert!(result.is_err());

    registration.unregister();
    db.unwind_if_revision_cancelled();
}
