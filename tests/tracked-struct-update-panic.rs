#![cfg(feature = "inventory")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

use salsa::plumbing::AsId;
use salsa::{Database, Setter};

static PANIC_ON_CHANGE: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Debug)]
struct PanicsOnce(u32);

impl PartialEq for PanicsOnce {
    fn eq(&self, other: &Self) -> bool {
        if self.0 != other.0 && PANIC_ON_CHANGE.swap(false, Ordering::Relaxed) {
            panic!("equality panic");
        }
        self.0 == other.0
    }
}

impl Eq for PanicsOnce {}

#[salsa::input]
struct Input {
    value: u32,
}

#[salsa::tracked]
struct Tracked<'db> {
    #[tracked]
    value: PanicsOnce,
}

#[salsa::tracked]
fn make_tracked(db: &dyn Database, input: Input) -> Tracked<'_> {
    Tracked::new(db, PanicsOnce(input.value(db)))
}

#[test]
fn tracked_struct_can_be_updated_after_panic() {
    PANIC_ON_CHANGE.store(true, Ordering::Relaxed);

    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 1);
    let tracked = make_tracked(&db, input);
    assert_eq!(tracked.value(&db).0, 1);

    input.set_value(&mut db).to(2);
    let result = catch_unwind(AssertUnwindSafe(|| make_tracked(&db, input)));
    assert!(result.is_err());

    let tracked = make_tracked(&db, input);
    assert_eq!(tracked.value(&db).0, 2);
}

static PANIC_AFTER_PARTIAL_UPDATE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
struct PanicsWhileEqual(u32);

impl PartialEq for PanicsWhileEqual {
    fn eq(&self, other: &Self) -> bool {
        if PANIC_AFTER_PARTIAL_UPDATE.swap(false, Ordering::Relaxed) {
            panic!("equality panic after an earlier field was updated");
        }
        self.0 == other.0
    }
}

impl Eq for PanicsWhileEqual {}

#[derive(Clone, Debug, salsa::Update)]
struct PartiallyPanickingValue {
    first: u32,
    second: PanicsWhileEqual,
}

#[salsa::tracked]
struct PartiallyUpdated<'db> {
    #[tracked]
    value: PartiallyPanickingValue,
}

#[salsa::tracked]
fn make_partially_updated(db: &dyn Database, input: Input) -> PartiallyUpdated<'_> {
    PartiallyUpdated::new(
        db,
        PartiallyPanickingValue {
            first: input.value(db),
            second: PanicsWhileEqual(0),
        },
    )
}

#[salsa::tracked]
fn read_first<'db>(db: &'db dyn Database, tracked: PartiallyUpdated<'db>) -> u32 {
    tracked.value(db).first
}

#[test]
fn partially_updated_struct_uses_fresh_slot_after_panic() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 1);
    let tracked = make_partially_updated(&db, input);
    let old_id = tracked.as_id();
    assert_eq!(read_first(&db, tracked), 1);

    input.set_value(&mut db).to(2);
    PANIC_AFTER_PARTIAL_UPDATE.store(true, Ordering::Relaxed);
    let result = catch_unwind(AssertUnwindSafe(|| make_partially_updated(&db, input)));
    assert!(result.is_err());

    let tracked = make_partially_updated(&db, input);
    assert_ne!(tracked.as_id(), old_id);
    assert_eq!(tracked.value(&db).first, 2);
    assert_eq!(read_first(&db, tracked), 2);
}
