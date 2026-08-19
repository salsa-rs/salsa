#![cfg(feature = "inventory")]
#![forbid(unsafe_code)]

use std::cell::Cell;
use std::panic::{self, catch_unwind};
use std::sync::Arc;

#[cfg(feature = "accumulator")]
use salsa::Accumulator;
use salsa::Database;

thread_local! {
    static DB: salsa::DatabaseImpl = salsa::DatabaseImpl::default();
    static REENTER_FROM_HOOK: Cell<bool> = const { Cell::new(false) };
    static HOOK_CALLS: Cell<usize> = const { Cell::new(0) };
    #[cfg(feature = "accumulator")]
    static DROPS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(feature = "accumulator")]
#[test]
fn rejected_accumulator_can_reenter() {
    DB.with(|db| {
        DROPS.set(0);
        let result = catch_unwind(|| Reenter.accumulate(db));
        assert_eq!(
            result.unwrap_err().downcast_ref::<&str>(),
            Some(&"cannot accumulate values outside of an active tracked function")
        );
        assert_eq!(DROPS.get(), 1);
        assert_eq!(read(db), 42);
    });
}

#[cfg(feature = "accumulator")]
#[test]
fn discarded_accumulator_can_reenter() {
    DB.with(|db| {
        DROPS.set(0);
        let result = catch_unwind(|| accumulate_then_panic(db));
        assert_eq!(
            result.unwrap_err().downcast_ref::<&str>(),
            Some(&"query panic")
        );
        assert_eq!(DROPS.get(), 1);
        assert_eq!(read(db), 42);
    });
}

#[test]
fn cycle_panic_hook_can_reenter() {
    // Hooks are process-wide. Only reenter on this test's thread, and preserve the existing hook
    // for panics in other tests.
    let previous_hook = Arc::new(panic::take_hook());
    let fallback = previous_hook.clone();
    panic::set_hook(Box::new(move |info| {
        if REENTER_FROM_HOOK.replace(false) {
            DB.with(|db| {
                db.report_untracked_read();
                assert_eq!(read(db), 42);
            });
            HOOK_CALLS.set(HOOK_CALLS.get() + 1);
        } else {
            fallback(info);
        }
    }));

    HOOK_CALLS.set(0);
    REENTER_FROM_HOOK.set(true);
    let result = catch_unwind(|| DB.with(|db| recursive(db)));
    REENTER_FROM_HOOK.set(false);

    // Restore the hook before making assertions that could panic.
    drop(panic::take_hook());
    panic::set_hook(Arc::try_unwrap(previous_hook).unwrap_or_else(|_| unreachable!()));

    let panic = result.unwrap_err();
    assert!(
        panic
            .downcast_ref::<String>()
            .unwrap()
            .contains("dependency graph cycle when querying")
    );
    assert_eq!(HOOK_CALLS.get(), 1);
    DB.with(|db| assert_eq!(read(db), 42));
}

#[salsa::tracked(returns(copy))]
fn read(_db: &dyn Database) -> u32 {
    42
}

#[salsa::tracked(returns(copy))]
fn recursive(db: &dyn Database) {
    recursive(db);
}

#[cfg(feature = "accumulator")]
#[salsa::accumulator]
#[derive(Debug)]
struct Reenter;

#[cfg(feature = "accumulator")]
impl Drop for Reenter {
    fn drop(&mut self) {
        DB.with(|db| {
            db.report_untracked_read();
            // This query can reuse the frame that is currently being discarded.
            assert_eq!(read(db), 42);
        });
        DROPS.set(DROPS.get() + 1);
    }
}

#[cfg(feature = "accumulator")]
#[salsa::tracked(returns(copy))]
fn accumulate_then_panic(db: &dyn Database) {
    Reenter.accumulate(db);
    panic!("query panic");
}
