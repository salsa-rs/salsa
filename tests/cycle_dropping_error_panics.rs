#![allow(warnings)]

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::atomic::AtomicUsize;

use expect_test::expect;
use salsa::Cycle;
use salsa::DatabaseImpl;
use salsa::Durability;

use salsa::Database as Db;
use salsa::Setter;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked(recovery_fn = recover_a)]
fn cycle_a(db: &dyn Db, input: MyInput) -> salsa::Result<String> {
    cycle_b(db, input)
}

fn recover_a(db: &dyn Db, cycle: &Cycle, input: MyInput) -> salsa::Result<String> {
    Ok("recovered".to_string())
}

#[salsa::tracked]
fn cycle_b(db: &dyn Db, input: MyInput) -> salsa::Result<String> {
    Ok(cycle_a(db, input).unwrap_or_else(|error| format!("Suppressed error: {error}")))
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "Cycle errors must be propagated so that Salsa can resolve the cycle.")]
fn execute() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = MyInput::new(db, 2);
        let result = cycle_a(db, input);

        panic!("Expected query to panic");
    })
}

#[salsa::tracked]
fn deferred_cycle_a(db: &dyn Db, input: MyInput) -> salsa::Result<String> {
    deferred_cycle_b(db, input)
}

// Simulates some global state in the database that is updated during a query.
// An example of this is an input-map.
static EVEN_COUNT: AtomicUsize = AtomicUsize::new(0);

#[salsa::tracked]
fn deferred_cycle_b(db: &dyn Db, input: MyInput) -> salsa::Result<String> {
    let is_even = input.field(db)? % 2 == 0;
    if is_even {
        EVEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    match deferred_cycle_c(db, input) {
        Ok(result) => Ok(result),
        Err(err) => {
            if is_even {
                EVEN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }

            Err(err)
        }
    }
}

#[salsa::tracked(recovery_fn = recover_c)]
fn deferred_cycle_c(db: &dyn Db, input: MyInput) -> salsa::Result<String> {
    deferred_cycle_a(db, input)
}

fn recover_c(db: &dyn Db, cycle: &Cycle, input: MyInput) -> salsa::Result<String> {
    Ok("recovered C".to_string())
}

// A query captures the error but propagates it before completion.
#[test]
fn deferred_propagation() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = MyInput::new(db, 2);
        let result = deferred_cycle_a(db, input);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "recovered C");

        assert_eq!(EVEN_COUNT.load(std::sync::atomic::Ordering::Relaxed), 1);
    })
}
