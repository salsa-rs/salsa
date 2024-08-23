// test for rayon interations.

use salsa::Cancelled;
use salsa::Setter;

use crate::setup::Knobs;
use crate::setup::KnobsDatabase;

#[salsa::input]
struct ParallelInput {
    field: Vec<u32>,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: ParallelInput) -> Vec<u32> {
    salsa::par_map(db, input.field(db), |_db, field| field + 1)
}

#[test]
fn execute() {
    let db = salsa::DatabaseImpl::new();

    let counts = (1..=10).collect::<Vec<u32>>();
    let input = ParallelInput::new(&db, counts);

    tracked_fn(&db, input);
}

#[salsa::tracked]
fn a1(db: &dyn KnobsDatabase, input: ParallelInput) -> Vec<u32> {
    db.signal(1);
    salsa::par_map(db, input.field(db), |db, field| {
        db.wait_for(2);
        field + 1
    })
}

#[salsa::tracked]
fn dummy(_db: &dyn KnobsDatabase, _input: ParallelInput) -> ParallelInput {
    panic!("should never get here!")
}

// we expect this to panic, as `salsa::par_map` needs to be called from a query.
#[test]
#[should_panic]
fn direct_calls_panic() {
    let db = salsa::DatabaseImpl::new();

    let counts = (1..=10).collect::<Vec<u32>>();
    let input = ParallelInput::new(&db, counts);
    let _: Vec<u32> = salsa::par_map(&db, input.field(&db), |_db, field| field + 1);
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
fn execute_cancellation() {
    let mut db = Knobs::default();

    let counts = (1..=10).collect::<Vec<u32>>();
    let input = ParallelInput::new(&db, counts);

    let thread_a = std::thread::spawn({
        let db = db.clone();
        move || a1(&db, input)
    });

    let counts = (2..=20).collect::<Vec<u32>>();

    db.signal_on_did_cancel.store(2);
    input.set_field(&mut db).to(counts);

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
