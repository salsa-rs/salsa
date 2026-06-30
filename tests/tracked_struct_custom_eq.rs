#![cfg(feature = "inventory")]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use salsa::{Database, Setter};

static MARK1: AtomicBool = AtomicBool::new(false);
static MARK2: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Hash, salsa::SalsaValue)]
struct CustomEq(usize);

#[salsa::tracked]
struct Tracked<'db> {
    #[tracked]
    #[eq(|old, new| {
        MARK1.store(true, Ordering::Release);
        old.0 == new.0
    })]
    tracked: CustomEq,
    #[eq(untracked_eq)]
    untracked: CustomEq,
}

fn untracked_eq(old: &CustomEq, new: &CustomEq) -> bool {
    MARK2.store(true, Ordering::Release);
    old.0 == new.0
}

#[salsa::input]
struct MyInput {
    #[returns(copy)]
    field1: usize,
    #[returns(copy)]
    field2: usize,
}

#[salsa::tracked(returns(copy))]
fn intermediate(db: &dyn salsa::Database, input: MyInput) -> Tracked<'_> {
    Tracked::new(db, CustomEq(input.field1(db)), CustomEq(input.field2(db)))
}

#[salsa::tracked(returns(copy))]
fn accumulate(db: &dyn salsa::Database, input: MyInput) -> (usize, usize) {
    let tracked = intermediate(db, input);
    let one = read_tracked(db, tracked);
    let two = read_untracked(db, tracked);

    (one, two)
}

#[salsa::tracked(returns(copy))]
fn read_tracked<'db>(db: &'db dyn Database, tracked: Tracked<'db>) -> usize {
    tracked.tracked(db).0
}

#[salsa::tracked(returns(copy))]
fn read_untracked<'db>(db: &'db dyn Database, tracked: Tracked<'db>) -> usize {
    tracked.untracked(db).0
}

#[test]
fn execute() {
    let mut db = salsa::DatabaseImpl::default();
    let input = MyInput::new(&db, 1, 1);

    assert_eq!(accumulate(&db, input), (1, 1));

    assert!(!MARK1.load(Ordering::Acquire));
    assert!(!MARK2.load(Ordering::Acquire));

    input.set_field1(&mut db).to(2);
    assert_eq!(accumulate(&db, input), (2, 1));

    assert!(MARK1.load(Ordering::Acquire));
    assert!(MARK2.load(Ordering::Acquire));
}
