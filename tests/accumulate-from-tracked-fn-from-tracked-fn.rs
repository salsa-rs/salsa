//! Accumulate values from within a tracked function.
//! Then mutate the values so that the tracked function re-executes.
//! Check that we accumulate the appropriate, new values.

use expect_test::expect;
use salsa::{Accumulator, Setter};
use test_log::test;

#[salsa::input(debug)]
struct List {
    value: u32,
    next: Option<List>,
}

// Silence a warning about this not being used (it is).
#[allow(unused)]
#[salsa::accumulator]
#[derive(Copy, Clone, Debug)]
struct Integers(u32);

#[salsa::tracked]
fn compute(db: &dyn salsa::Database, input: List) {
    compute_single(db, input);
    if let Some(next) = input.next(db) {
        compute(db, next);
    }
}

// In https://github.com/salsa-rs/salsa/issues/923 there was an issue specifically with tracked fn calling tracked fn.
#[salsa::tracked]
fn compute_single(db: &dyn salsa::Database, input: List) {
    Integers(input.value(db)).accumulate(db);
}

#[test]
fn test1() {
    let mut db = salsa::DatabaseImpl::new();

    let l0 = List::new(&db, 1, None);
    let l1 = List::new(&db, 10, Some(l0));

    compute(&db, l1);
    expect![[r#"
        [
            Integers(
                10,
            ),
            Integers(
                1,
            ),
        ]
    "#]]
    .assert_debug_eq(&compute::accumulated::<Integers>(&db, l1));

    l0.set_value(&mut db).to(2);
    compute(&db, l1);
    expect![[r#"
        [
            Integers(
                10,
            ),
            Integers(
                2,
            ),
        ]
    "#]]
    .assert_debug_eq(&compute::accumulated::<Integers>(&db, l1));

    l1.set_value(&mut db).to(11);
    compute(&db, l1);
    expect![[r#"
        [
            Integers(
                11,
            ),
            Integers(
                2,
            ),
        ]
    "#]]
    .assert_debug_eq(&compute::accumulated::<Integers>(&db, l1));
}
