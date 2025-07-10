#![cfg(feature = "inventory")]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use salsa::{Database, Setter};

static MARK1: AtomicBool = AtomicBool::new(false);
static MARK2: AtomicBool = AtomicBool::new(false);

#[salsa::tracked]
struct Tracked<'db> {
    #[tracked]
    #[maybe_update(|dst, src| {
        *dst = src;
        MARK1.store(true, Ordering::Release);
        true
    })]
    tracked: usize,
    #[maybe_update(untracked_update)]
    untracked: usize,
}

unsafe fn untracked_update(dst: *mut usize, src: usize) -> bool {
    unsafe { *dst = src };
    MARK2.store(true, Ordering::Release);
    true
}

#[salsa::input]
struct MyInput {
    field1: usize,
    field2: usize,
}

#[salsa::tracked]
fn intermediate(db: &dyn salsa::Database, input: MyInput) -> Tracked<'_> {
    Tracked::new(db, input.field1(db), input.field2(db))
}

#[salsa::tracked]
fn accumulate(db: &dyn salsa::Database, input: MyInput) -> (usize, usize) {
    let tracked = intermediate(db, input);
    let one = read_tracked(db, tracked);
    let two = read_untracked(db, tracked);

    (one, two)
}

#[salsa::tracked]
fn read_tracked<'db>(db: &'db dyn Database, tracked: Tracked<'db>) -> usize {
    tracked.tracked(db)
}

#[salsa::tracked]
fn read_untracked<'db>(db: &'db dyn Database, tracked: Tracked<'db>) -> usize {
    tracked.untracked(db)
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
