#![cfg(feature = "inventory")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};

use salsa::{Database, Setter};

static PANIC_ON_CHANGE: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Debug, Hash)]
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
