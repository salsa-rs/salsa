use crate::setup::{Input, Knobs, ParDatabase, ParDatabaseImpl, WithValue};
use salsa::{Database, ParallelDatabase};

/// Add test where a call to `sum` is cancelled by a simultaneous
/// write. Check that we recompute the result in next revision, even
/// though none of the inputs have changed.
#[test]
fn in_par_get_set_cancellation() {
    let db = ParDatabaseImpl::default();

    db.query(Input).set('a', 100);
    db.query(Input).set('b', 010);
    db.query(Input).set('c', 001);
    db.query(Input).set('d', 0);

    let thread1 = std::thread::spawn({
        let db = db.fork();
        move || {
            let v1 = db.sum_signal_on_entry().with_value(1, || {
                db.sum_await_cancellation()
                    .with_value(true, || db.sum("abc"))
            });

            // check that we observed cancellation
            assert_eq!(v1, std::usize::MAX);

            // at this point, we have observed cancellation, so let's
            // wait until the `set` is known to have occurred.
            db.signal().await(2);

            // Now when we read we should get the correct sums. Note
            // in particular that we re-compute the sum of `"abc"`
            // even though none of our inputs have changed.
            let v2 = db.sum("abc");
            (v1, v2)
        }
    });

    let thread2 = std::thread::spawn({
        let db = db.fork();
        move || {
            // Wait until we have entered `sum` in the other thread.
            db.signal().await(1);

            db.query(Input).set('d', 1000);

            // Signal that we have *set* `d`
            db.signal().signal(2);

            db.sum("d")
        }
    });

    assert_eq!(thread1.join().unwrap(), (std::usize::MAX, 111));
    assert_eq!(thread2.join().unwrap(), 1000);
}
