use crate::setup::{ParDatabase, ParDatabaseImpl};
use crate::signal::Signal;
use salsa::{Database, ParallelDatabase};
use std::sync::Arc;

/// Add test where a call to `sum` is cancelled by a simultaneous
/// write. Check that we recompute the result in next revision, even
/// though none of the inputs have changed.
#[test]
fn in_par_get_set_cancellation() {
    let mut db = ParDatabaseImpl::default();

    db.set_input('a', 1);

    let signal = Arc::new(Signal::default());

    let thread1 = std::thread::spawn({
        let db = db.snapshot();
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

            // Since this is a snapshotted database, we are in a consistent
            // revision, so this must yield the same value.
            let w = db.input('a');

            (v, w)
        }
    });

    let thread2 = std::thread::spawn({
        let signal = signal.clone();
        move || {
            // Wait until thread 1 has asserted that they are not cancelled
            // before we invoke `set.`
            signal.wait_for(1);

            // This will block until thread1 drops the revision lock.
            db.set_input('a', 2);

            db.input('a')
        }
    });

    let (a, b) = thread1.join().unwrap();
    assert_eq!(a, 1);
    assert_eq!(b, 1);

    let c = thread2.join().unwrap();
    assert_eq!(c, 2);
}
