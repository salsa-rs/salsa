//! Test that suppressing a cancellation error inside a query
//! panics in debug mode.

use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use salsa::Setter;

use crate::setup::Knobs;
use crate::setup::KnobsDatabase;

#[salsa::input]
struct MyInput {
    field: i32,
}

#[salsa::tracked]
fn a1(db: &dyn KnobsDatabase, input: MyInput) -> salsa::Result<String> {
    db.signal(1);
    db.wait_for(2);

    match dummy(db, input) {
        Ok(result) => Ok(result),
        Err(_) => Ok("Suppressed cancellation".to_string()),
    }
}

#[salsa::tracked]
fn dummy(_db: &dyn KnobsDatabase, _input: MyInput) -> salsa::Result<String> {
    Ok("should never get here!".to_string())
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
// drops error -> panics

#[test]
#[cfg(debug_assertions)]
fn execute() {
    let mut db = Knobs::default();

    let input = MyInput::new(&db, 1);

    let thread_a = std::thread::Builder::new()
        .name("a".to_string())
        .spawn({
            let db = db.clone();
            move || a1(&db, input)
        })
        .unwrap();

    db.wait_for(1);
    db.signal_on_did_cancel.store(2);
    input.set_field(&mut db).to(2);

    // Assert thread A panicked because it captured the error
    let error = thread_a.join().unwrap_err();

    if let Some(error) = error.downcast_ref::<String>() {
        assert_eq!(*error, "Cancellation errors must be propagated inside salsa queries. If you see this message outside a salsa query, please open an issue.");
    } else {
        panic!("Thread A should have panicked!")
    }
}

// Simulates some global state in the database that is updated during a query.
// An example of this is an input-map.
static EVEN_COUNT: AtomicUsize = AtomicUsize::new(0);

#[salsa::tracked]
fn a1_deferred(db: &dyn KnobsDatabase, input: MyInput) -> salsa::Result<String> {
    let is_even = input.field(db)? % 2 == 0;
    if is_even {
        EVEN_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    db.signal(1);
    db.wait_for(2);

    match dummy(db, input) {
        Ok(result) => Ok(result),
        Err(error) => {
            if is_even {
                EVEN_COUNT.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }

            Err(error)
        }
    }
}

#[test]
fn rethrow() {
    let mut db = Knobs::default();

    let input = MyInput::new(&db, 2);

    let thread_a = std::thread::Builder::new()
        .name("a".to_string())
        .spawn({
            let db = db.clone();
            move || a1_deferred(&db, input)
        })
        .unwrap();

    db.wait_for(1);
    db.signal_on_did_cancel.store(2);
    input.set_field(&mut db).to(2);

    // Assert thread A was cancelled.
    let result = thread_a.join().unwrap();

    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().to_string(),
        "cancelled because of pending write"
    );
    assert_eq!(EVEN_COUNT.load(Ordering::Relaxed), 0);
}
