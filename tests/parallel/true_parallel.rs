use crate::setup::{Input, Knobs, ParDatabase, ParDatabaseImpl, WithValue};
use salsa::{Database, ParallelDatabase};

/// Test where two threads are executing sum. We show that they can
/// both be executing sum in parallel by having thread1 wait for
/// thread2 to send a signal before it leaves (similarly, thread2
/// waits for thread1 to send a signal before it enters).
#[test]
fn true_parallel_different_keys() {
    let db = ParDatabaseImpl::default();

    db.query(Input).set('a', 100);
    db.query(Input).set('b', 010);
    db.query(Input).set('c', 001);

    // Thread 1 will signal stage 1 when it enters and wait for stage 2.
    let thread1 = std::thread::spawn({
        let db = db.fork();
        move || {
            let v = db.knobs().sum_signal_on_entry.with_value(1, || {
                db.knobs()
                    .sum_wait_for_on_exit
                    .with_value(2, || db.sum("a"))
            });
            v
        }
    });

    // Thread 2 will wait_for stage 1 when it enters and signal stage 2
    // when it leaves.
    let thread2 = std::thread::spawn({
        let db = db.fork();
        move || {
            let v = db.knobs().sum_wait_for_on_entry.with_value(1, || {
                db.knobs().sum_signal_on_exit.with_value(2, || db.sum("b"))
            });
            v
        }
    });

    assert_eq!(thread1.join().unwrap(), 100);
    assert_eq!(thread2.join().unwrap(), 010);
}

/// Add a test that tries to trigger a conflict, where we fetch
/// `sum("abc")` from two threads simultaneously, and of them
/// therefore has to block.
#[test]
fn true_parallel_same_keys() {
    let db = ParDatabaseImpl::default();

    db.query(Input).set('a', 100);
    db.query(Input).set('b', 010);
    db.query(Input).set('c', 001);

    // Thread 1 will wait_for a barrier in the start of `sum`
    let thread1 = std::thread::spawn({
        let db = db.fork();
        move || {
            let v = db.knobs().sum_signal_on_entry.with_value(1, || {
                db.knobs()
                    .sum_wait_for_on_entry
                    .with_value(2, || db.sum("abc"))
            });
            v
        }
    });

    // Thread 2 will wait until Thread 1 has entered sum and then --
    // once it has set itself to block -- signal Thread 1 to
    // continue. This way, we test out the mechanism of one thread
    // blocking on another.
    let thread2 = std::thread::spawn({
        let db = db.fork();
        move || {
            db.knobs().signal.wait_for(1);
            db.knobs().signal_on_will_block.set(2);
            db.sum("abc")
        }
    });

    assert_eq!(thread1.join().unwrap(), 111);
    assert_eq!(thread2.join().unwrap(), 111);
}
