use crate::setup::{Input, ParDatabase, ParDatabaseImpl};
use crate::signal::Signal;
use salsa::{Database, ParallelDatabase};
use std::sync::Arc;

/// Add test where a call to `sum` is cancelled by a simultaneous
/// write. Check that we recompute the result in next revision, even
/// though none of the inputs have changed.
#[test]
fn in_par_get_set_cancellation() {
    let db = ParDatabaseImpl::default();

    db.query(Input).set('a', 1);

    let signal = Arc::new(Signal::default());

    let lock = db.salsa_runtime().lock_revision();
    let thread1 = std::thread::spawn({
        let db = db.fork_mut();
        let signal = signal.clone();
        move || {
            // Check that cancellation flag is not yet set, because
            // `set` cannot have been called yet.
            assert!(!db.salsa_runtime().is_current_revision_canceled());

            // Signal other thread to proceed.
            signal.signal(1);

            // Wait for other thread to signal cancellation
            while !db.salsa_runtime().is_current_revision_canceled() {
                std::thread::yield_now();
            }

            // Since we have not yet released revision lock, we should
            // see 1 here.
            let v = db.input('a');

            // Release the lock.
            std::mem::drop(lock);

            // This could come before or after the `set` in the other
            // thread.
            let w = db.input('a');

            (v, w)
        }
    });

    let thread2 = std::thread::spawn({
        let db = db.fork_mut();
        let signal = signal.clone();
        move || {
            // Wait until thread 1 has asserted that they are not cancelled
            // before we invoke `set.`
            signal.wait_for(1);

            // This will block until thread1 drops the revision lock.
            db.query(Input).set('a', 2);

            db.input('a')
        }
    });

    // The first read is done with the revision lock, so it *must* see
    // `1`; the second read could see either `1` or `2`.
    let (a, b) = thread1.join().unwrap();
    assert_eq!(a, 1);
    assert!(b == 1 || b == 2, "saw unexpected value for b: {}", b);

    let c = thread2.join().unwrap();
    assert_eq!(c, 2);
}
