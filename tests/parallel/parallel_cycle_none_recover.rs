//! Test a cycle where no queries recover that occurs across threads.
//! See the `../cycles.rs` for a complete listing of cycle tests,
//! both intra and cross thread.

use crate::setup::Database;
use crate::setup::Knobs;
use expect_test::expect;
use salsa::ParallelDatabase;

pub(crate) trait Db: salsa::DbWithJar<Jar> + Knobs {}

impl<T: salsa::DbWithJar<Jar> + Knobs> Db for T {}

#[salsa::jar(db = Db)]
pub(crate) struct Jar(MyInput, a, b);

#[salsa::input(jar = Jar)]
pub(crate) struct MyInput {
    field: i32,
}

#[salsa::tracked(jar = Jar)]
pub(crate) fn a(db: &dyn Db, input: MyInput) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.signal(1);
    db.wait_for(2);

    b(db, input)
}

#[salsa::tracked(jar = Jar)]
pub(crate) fn b(db: &dyn Db, input: MyInput) -> i32 {
    // Wait to create the cycle until both threads have entered
    db.wait_for(1);
    db.signal(2);

    // Wait for thread A to block on this thread
    db.wait_for(3);

    // Now try to execute A
    a(db, input)
}

#[test]
fn execute() {
    let db = Database::default();
    db.knobs().signal_on_will_block.set(3);

    let input = MyInput::new(&db, -1);

    let thread_a = std::thread::spawn({
        let db = db.snapshot();
        move || a(&*db, input)
    });

    let thread_b = std::thread::spawn({
        let db = db.snapshot();
        move || b(&*db, input)
    });

    // We expect B to panic because it detects a cycle (it is the one that calls A, ultimately).
    // Right now, it panics with a string.
    let err_b = thread_b.join().unwrap_err();
    if let Some(c) = err_b.downcast_ref::<salsa::Cycle>() {
        let expected = expect![[r#"
            [
                "a(0)",
                "b(0)",
            ]
        "#]];
        expected.assert_debug_eq(&c.all_participants(&db));
    } else {
        panic!("b failed in an unexpected way: {:?}", err_b);
    }

    // We expect A to propagate a panic, which causes us to use the sentinel
    // type `Canceled`.
    assert!(thread_a
        .join()
        .unwrap_err()
        .downcast_ref::<salsa::Cancelled>()
        .is_some());
}
