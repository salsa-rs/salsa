use crate::setup::{CancelationFlag, Canceled, Knobs, ParDatabase, ParDatabaseImpl, WithValue};
use salsa::ParallelDatabase;

macro_rules! assert_canceled {
    ($flag:expr, $thread:expr) => {
        if $flag == CancelationFlag::Panic {
            match $thread.join() {
                Ok(value) => panic!("expected cancelation, got {:?}", value),
                Err(payload) => match payload.downcast::<Canceled>() {
                    Ok(_) => {}
                    Err(payload) => ::std::panic::resume_unwind(payload),
                },
            }
        } else {
            assert_eq!($thread.join().unwrap(), usize::max_value());
        }
    };
}

/// We have to falvors of cancellation: based on unwindig and based on anon
/// reads. This checks both,
fn check_cancelation(f: impl Fn(CancelationFlag)) {
    f(CancelationFlag::Panic);
    f(CancelationFlag::SpecialValue);
}

/// Add test where a call to `sum` is cancelled by a simultaneous
/// write. Check that we recompute the result in next revision, even
/// though none of the inputs have changed.
#[test]
fn in_par_get_set_cancellation_immediate() {
    check_cancelation(|flag| {
        let mut db = ParDatabaseImpl::default();

        db.set_input('a', 100);
        db.set_input('b', 010);
        db.set_input('c', 001);
        db.set_input('d', 0);

        let thread1 = std::thread::spawn({
            let db = db.snapshot();
            move || {
                // This will not return until it sees cancellation is
                // signaled.
                db.knobs().sum_signal_on_entry.with_value(1, || {
                    db.knobs()
                        .sum_wait_for_cancellation
                        .with_value(flag, || db.sum("abc"))
                })
            }
        });

        // Wait until we have entered `sum` in the other thread.
        db.wait_for(1);

        // Try to set the input. This will signal cancellation.
        db.set_input('d', 1000);

        // This should re-compute the value (even though no input has changed).
        let thread2 = std::thread::spawn({
            let db = db.snapshot();
            move || db.sum("abc")
        });

        assert_eq!(db.sum("d"), 1000);
        assert_canceled!(flag, thread1);
        assert_eq!(thread2.join().unwrap(), 111);
    })
}

/// Here, we check that `sum`'s cancellation is propagated
/// to `sum2` properly.
#[test]
fn in_par_get_set_cancellation_transitive() {
    check_cancelation(|flag| {
        let mut db = ParDatabaseImpl::default();

        db.set_input('a', 100);
        db.set_input('b', 010);
        db.set_input('c', 001);
        db.set_input('d', 0);

        let thread1 = std::thread::spawn({
            let db = db.snapshot();
            move || {
                // This will not return until it sees cancellation is
                // signaled.
                db.knobs().sum_signal_on_entry.with_value(1, || {
                    db.knobs()
                        .sum_wait_for_cancellation
                        .with_value(flag, || db.sum2("abc"))
                })
            }
        });

        // Wait until we have entered `sum` in the other thread.
        db.wait_for(1);

        // Try to set the input. This will signal cancellation.
        db.set_input('d', 1000);

        // This should re-compute the value (even though no input has changed).
        let thread2 = std::thread::spawn({
            let db = db.snapshot();
            move || db.sum2("abc")
        });

        assert_eq!(db.sum2("d"), 1000);
        assert_canceled!(flag, thread1);
        assert_eq!(thread2.join().unwrap(), 111);
    })
}

/// https://github.com/salsa-rs/salsa/issues/66
#[test]
fn no_back_dating_in_cancellation() {
    check_cancelation(|flag| {
        let mut db = ParDatabaseImpl::default();

        db.set_input('a', 1);
        let thread1 = std::thread::spawn({
            let db = db.snapshot();
            move || {
                // Here we compute a long-chain of queries,
                // but the last one gets cancelled.
                db.knobs().sum_signal_on_entry.with_value(1, || {
                    db.knobs()
                        .sum_wait_for_cancellation
                        .with_value(flag, || db.sum3("a"))
                })
            }
        });

        db.wait_for(1);

        // Set unrelated input to bump revision
        db.set_input('b', 2);

        // Here we should recompuet the whole chain again, clearing the cancellation
        // state. If we get `usize::max()` here, it is a bug!
        assert_eq!(db.sum3("a"), 1);

        assert_canceled!(flag, thread1);

        db.set_input('a', 3);
        db.set_input('a', 4);
        assert_eq!(db.sum3("ab"), 6);
    })
}

/// Here, we compute `sum3_drop_sum` and -- in the process -- observe
/// a cancellation. As a result, we have to recompute `sum` when we
/// reinvoke `sum3_drop_sum` and we have to re-execute
/// `sum2_drop_sum`.  But the result of `sum2_drop_sum` doesn't
/// change, so we don't have to re-execute `sum3_drop_sum`.
/// This only works with SpecialValue cancellation strategy.
#[test]
fn transitive_cancellation() {
    let mut db = ParDatabaseImpl::default();

    db.set_input('a', 1);
    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || {
            // Here we compute a long-chain of queries,
            // but the last one gets cancelled.
            db.knobs().sum_signal_on_entry.with_value(1, || {
                db.knobs()
                    .sum_wait_for_cancellation
                    .with_value(CancelationFlag::SpecialValue, || db.sum3_drop_sum("a"))
            })
        }
    });

    db.wait_for(1);

    db.set_input('b', 2);

    // Check that when we call `sum3_drop_sum` we don't wind up having
    // to actually re-execute it, because the result of `sum2` winds
    // up not changing.
    db.knobs().sum3_drop_sum_should_panic.with_value(true, || {
        assert_eq!(db.sum3_drop_sum("a"), 22);
    });

    assert_eq!(thread1.join().unwrap(), 22);
}
