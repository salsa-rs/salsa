#![cfg(feature = "inventory")]

use salsa::{Database, Setter};

#[salsa::tracked]
fn memoized(db: &dyn Database, input: MyInput) -> u32 {
    memoized_a(db, MyTracked::new(db, input.field(db)))
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn memoized_a<'db>(db: &'db dyn Database, tracked: MyTracked<'db>) -> u32 {
    MyTracked::new(db, 0);
    memoized_b(db, tracked)
}

fn cycle_fn<'db>(
    _db: &'db dyn Database,
    _value: &u32,
    _count: u32,
    _input: MyTracked<'db>,
) -> salsa::CycleRecoveryAction<u32> {
    salsa::CycleRecoveryAction::Iterate
}

fn cycle_initial(_db: &dyn Database, _input: MyTracked) -> u32 {
    0
}

#[salsa::tracked]
fn memoized_b<'db>(db: &'db dyn Database, tracked: MyTracked<'db>) -> u32 {
    let incr = tracked.field(db);
    let a = memoized_a(db, tracked);
    if a > 8 {
        a
    } else {
        a + incr
    }
}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[test]
fn cycle_memoized() {
    let mut db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 2);
    assert_eq!(memoized(&db, input), 10);
    input.set_field(&mut db).to(3);
    assert_eq!(memoized(&db, input), 9);
}
