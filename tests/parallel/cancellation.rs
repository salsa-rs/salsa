use crate::setup::{Input, Knobs, ParDatabase, ParDatabaseImpl, WithValue};
use salsa::{Database, ParallelDatabase};

/// Add test where a call to `sum` is cancelled by a simultaneous
/// write. Check that we recompute the result in next revision, even
/// though none of the inputs have changed.
#[test]
fn in_par_get_set_cancellation_immediate() {
    let mut db = ParDatabaseImpl::default();

    db.query_mut(Input).set('a', 100);
    db.query_mut(Input).set('b', 010);
    db.query_mut(Input).set('c', 001);
    db.query_mut(Input).set('d', 0);

    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || {
            // This will not return until it sees cancellation is
            // signaled.
            db.knobs().sum_signal_on_entry.with_value(1, || {
                db.knobs()
                    .sum_wait_for_cancellation
                    .with_value(true, || db.sum("abc"))
            })
        }
    });

    // Wait until we have entered `sum` in the other thread.
    db.wait_for(1);

    // Try to set the input. This will signal cancellation.
    db.query_mut(Input).set('d', 1000);

    // This should re-compute the value (even though no input has changed).
    let thread2 = std::thread::spawn({
        let db = db.snapshot();
        move || db.sum("abc")
    });

    assert_eq!(db.sum("d"), 1000);
    assert_eq!(thread1.join().unwrap(), std::usize::MAX);
    assert_eq!(thread2.join().unwrap(), 111);
}

/// Here, we check that `sum`'s cancellation is propagated
/// to `sum2` properly.
#[test]
fn in_par_get_set_cancellation_transitive() {
    let mut db = ParDatabaseImpl::default();

    db.query_mut(Input).set('a', 100);
    db.query_mut(Input).set('b', 010);
    db.query_mut(Input).set('c', 001);
    db.query_mut(Input).set('d', 0);

    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || {
            // This will not return until it sees cancellation is
            // signaled.
            db.knobs().sum_signal_on_entry.with_value(1, || {
                db.knobs()
                    .sum_wait_for_cancellation
                    .with_value(true, || db.sum2("abc"))
            })
        }
    });

    // Wait until we have entered `sum` in the other thread.
    db.wait_for(1);

    // Try to set the input. This will signal cancellation.
    db.query_mut(Input).set('d', 1000);

    // This should re-compute the value (even though no input has changed).
    let thread2 = std::thread::spawn({
        let db = db.snapshot();
        move || db.sum2("abc")
    });

    assert_eq!(db.sum2("d"), 1000);
    assert_eq!(thread1.join().unwrap(), std::usize::MAX);
    assert_eq!(thread2.join().unwrap(), 111);
}

/// https://github.com/salsa-rs/salsa/issues/66
#[test]
fn no_back_dating_in_cancellation() {
    let mut db = ParDatabaseImpl::default();

    db.query_mut(Input).set('a', 1);
    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || {
            // Here we compute a long-chain of queries,
            // but the last one gets cancelled.
            db.knobs().sum_signal_on_entry.with_value(1, || {
                db.knobs()
                    .sum_wait_for_cancellation
                    .with_value(true, || db.sum3("a"))
            })
        }
    });

    db.wait_for(1);
    // Set unrelated input to bump revision.
    db.query_mut(Input).set('b', 2);

    // Here we should recompute the whole chain again, clearing the cancellation
    // state. If we get `usize::max()` here, it is a bug!
    assert_eq!(db.sum3("a"), 1);

    assert_eq!(thread1.join().unwrap(), std::usize::MAX);
}
