use crate::db;
use crate::group::*;
use salsa::debug::DebugQueryTable;
use salsa::{Database, SweepStrategy};

macro_rules! assert_keys {
    ($db:expr, $($query:expr => ($($key:expr),*),)*) => {
        $(
            let mut keys = $db.query($query).keys::<Vec<_>>();
            keys.sort();
            assert_eq!(keys, vec![$($key),*], "query {:?} had wrong keys", $query);
        )*
    };
}

#[test]
fn compute_one() {
    let db = db::DatabaseImpl::default();

    // Will compute fibonacci(5)
    db.query(UseTriangular).set(5, false);
    db.compute(5);

    db.salsa_runtime().next_revision();

    assert_keys! {
        db,
        Triangular => (),
        Fibonacci => (0, 1, 2, 3, 4, 5),
        Compute => (5),
        UseTriangular => (5),
        Min => (),
        Max => (),
    }

    // Memoized, but will compute fibonacci(5) again
    db.compute(5);
    db.sweep_all(SweepStrategy::default());

    assert_keys! {
        db,
        Triangular => (),
        Fibonacci => (5),
        Compute => (5),
        UseTriangular => (5),
        Min => (),
        Max => (),
    }
}

#[test]
fn compute_switch() {
    let db = db::DatabaseImpl::default();

    // Will compute fibonacci(5)
    db.query(UseTriangular).set(5, false);
    assert_eq!(db.compute(5), 5);

    // Change to triangular mode
    db.query(UseTriangular).set(5, true);

    // Now computes triangular(5)
    assert_eq!(db.compute(5), 15);

    // We still have entries for Fibonacci, even though they
    // are not relevant to the most recent value of `Compute`
    assert_keys! {
        db,
        Triangular => (0, 1, 2, 3, 4, 5),
        Fibonacci => (0, 1, 2, 3, 4, 5),
        Compute => (5),
        UseTriangular => (5),
        Min => (),
        Max => (),
    }

    db.sweep_all(SweepStrategy::default());

    // Now we just have `Triangular` and not `Fibonacci`
    assert_keys! {
        db,
        Triangular => (0, 1, 2, 3, 4, 5),
        Fibonacci => (),
        Compute => (5),
        UseTriangular => (5),
        Min => (),
        Max => (),
    }

    // Now run `compute` *again* in next revision.
    db.salsa_runtime().next_revision();
    assert_eq!(db.compute(5), 15);
    db.sweep_all(SweepStrategy::default());

    // We keep triangular, but just the outermost one.
    assert_keys! {
        db,
        Triangular => (5),
        Fibonacci => (),
        Compute => (5),
        UseTriangular => (5),
        Min => (),
        Max => (),
    }
}

/// Test a query with multiple layers of keys.
#[test]
fn compute_all() {
    let db = db::DatabaseImpl::default();

    for i in 0..6 {
        db.query(UseTriangular).set(i, (i % 2) != 0);
    }

    db.query(Min).set((), 0);
    db.query(Max).set((), 6);

    db.compute_all();
    db.salsa_runtime().next_revision();
    db.compute_all();
    db.sweep_all(SweepStrategy::default());

    assert_keys! {
        db,
        Triangular => (1, 3, 5),
        Fibonacci => (0, 2, 4),
        Compute => (0, 1, 2, 3, 4, 5),
        ComputeAll => (()),
        UseTriangular => (0, 1, 2, 3, 4, 5),
        Min => (()),
        Max => (()),
    }

    // Reduce the range to exclude index 5.
    db.query(Max).set((), 5);
    db.compute_all();

    assert_keys! {
        db,
        Triangular => (1, 3, 5),
        Fibonacci => (0, 2, 4),
        Compute => (0, 1, 2, 3, 4, 5),
        ComputeAll => (()),
        UseTriangular => (0, 1, 2, 3, 4, 5),
        Min => (()),
        Max => (()),
    }

    db.sweep_all(SweepStrategy::default());

    // We no longer used `Compute(5)` and `Triangular(5)`; note that
    // `UseTriangular(5)` is not collected, as it is an input.
    assert_keys! {
        db,
        Triangular => (1, 3),
        Fibonacci => (0, 2, 4),
        Compute => (0, 1, 2, 3, 4),
        ComputeAll => (()),
        UseTriangular => (0, 1, 2, 3, 4, 5),
        Min => (()),
        Max => (()),
    }
}
