#![cfg(feature = "rayon")]
// test for rayon-like scope interactions.

use salsa::{Cancelled, Setter};

use crate::setup::{Knobs, KnobsDatabase};

#[salsa::input]
struct ParallelInput {
    a: u32,
    b: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: ParallelInput) -> (u32, u32) {
    let mut a = None;
    let mut b = None;
    salsa::scope(db, |scope| {
        scope.spawn(|scope| a = Some(input.a(scope.db()) + 1));
        scope.spawn(|scope| b = Some(input.b(scope.db()) + 1));
    });
    (a.unwrap(), b.unwrap())
}

#[salsa::tracked]
fn a1(db: &dyn KnobsDatabase, input: ParallelInput) -> (u32, u32) {
    db.signal(1);
    let mut a = None;
    let mut b = None;
    salsa::scope(db, |scope| {
        scope.spawn(|scope| {
            scope.db().wait_for(2);
            a = Some(input.a(scope.db()) + 1)
        });
        scope.spawn(|scope| {
            scope.db().wait_for(2);
            b = Some(input.b(scope.db()) + 1)
        });
    });
    (a.unwrap(), b.unwrap())
}

#[salsa::tracked]
fn dummy(_db: &dyn KnobsDatabase) -> u32 {
    panic!("should never get here!")
}

#[test]
#[cfg_attr(miri, ignore)]
fn execute() {
    let db = salsa::DatabaseImpl::new();

    let input = ParallelInput::new(&db, 10, 20);

    tracked_fn(&db, input);
}

// we expect this to panic, as `salsa::par_map` needs to be called from a query.
#[test]
#[cfg_attr(miri, ignore)]
#[should_panic]
fn direct_calls_panic() {
    let db = salsa::DatabaseImpl::new();

    let input = ParallelInput::new(&db, 10, 20);
    salsa::scope(&db, |scope| {
        scope.spawn(|scope| _ = input.a(scope.db()) + 1);
        scope.spawn(|scope| _ = input.b(scope.db()) + 1);
    });
}

// Cancellation signalling test
//
// The pattern is as follows.
//
// Thread A                   Thread B
// --------                   --------
// a1
// |                          wait for stage 1
// signal stage 1             set input, triggers cancellation
// wait for stage 2 (blocks)  triggering cancellation sends stage 2
// |
// (unblocked)
// dummy
// panics

#[test]
#[cfg_attr(miri, ignore)]
fn execute_cancellation() {
    let mut db = Knobs::default();

    let input = ParallelInput::new(&db, 10, 20);

    let thread_a = std::thread::spawn({
        let db = db.clone();
        move || a1(&db, input)
    });

    db.signal_on_did_cancel(2);
    input.set_a(&mut db).to(30);

    // Assert thread A was cancelled
    let cancelled = thread_a
        .join()
        .unwrap_err()
        .downcast::<Cancelled>()
        .unwrap();

    // and inspect the output
    expect_test::expect![[r#"
        PendingWrite
    "#]]
    .assert_debug_eq(&cancelled);
}
