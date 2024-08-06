//! Test for cycle recover spread across two threads.
//! See `../cycles.rs` for a complete listing of cycle tests,
//! both intra and cross thread.

use salsa::Cancelled;
use salsa::Setter;

use crate::setup::Knobs;
use crate::setup::KnobsDatabase;

#[salsa::input]
struct MyInput {
    field: i32,
}

#[salsa::tracked]
fn a1(db: &dyn KnobsDatabase, input: MyInput) -> MyInput {
    db.signal(1);
    db.wait_for(2);
    dummy(db, input)
}

#[salsa::tracked]
fn dummy(_db: &dyn KnobsDatabase, _input: MyInput) -> MyInput {
    panic!("should never get here!")
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
fn execute() {
    let mut db = Knobs::default();

    let input = MyInput::new(&db, 1);

    let thread_a = std::thread::spawn({
        let db = db.clone();
        move || a1(&db, input)
    });

    db.signal_on_did_cancel.store(2);
    input.set_field(&mut db).to(2);

    // Assert thread A *should* was cancelled
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
