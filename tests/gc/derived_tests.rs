use crate::db;
use crate::group::*;
use salsa::debug::DebugQueryTable;
use salsa::{Database, Durability, SweepStrategy};

#[test]
fn compute_one_write_low() {
    let mut db = db::DatabaseImpl::default();

    // Will compute fibonacci(5)
    db.set_use_triangular(5, false);
    db.compute(5);

    db.salsa_runtime_mut().synthetic_write(Durability::LOW);

    assert_keys! {
        db,
        TriangularQuery => (),
        FibonacciQuery => (0, 1, 2, 3, 4, 5),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }

    // Memoized, but will compute fibonacci(5) again
    db.compute(5);
    db.sweep_all(SweepStrategy::discard_outdated());

    assert_keys! {
        db,
        TriangularQuery => (),
        FibonacciQuery => (5),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }
}

#[test]
fn compute_one_write_high() {
    let mut db = db::DatabaseImpl::default();

    // Will compute fibonacci(5)
    db.set_use_triangular(5, false);
    db.compute(5);

    // Doing a synthetic write with durability *high* means that we
    // will revalidate the things `compute(5)` uses, and hence they
    // are not discarded.
    db.salsa_runtime_mut().synthetic_write(Durability::HIGH);

    assert_keys! {
        db,
        TriangularQuery => (),
        FibonacciQuery => (0, 1, 2, 3, 4, 5),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }

    // Memoized, but will compute fibonacci(5) again
    db.compute(5);
    db.sweep_all(SweepStrategy::discard_outdated());

    assert_keys! {
        db,
        TriangularQuery => (),
        FibonacciQuery => (0, 1, 2, 3, 4, 5),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }
}

#[test]
fn compute_switch() {
    let mut db = db::DatabaseImpl::default();

    // Will compute fibonacci(5)
    db.set_use_triangular(5, false);
    assert_eq!(db.compute(5), 5);

    // Change to triangular mode
    db.set_use_triangular(5, true);

    // Now computes triangular(5)
    assert_eq!(db.compute(5), 15);

    // We still have entries for Fibonacci, even though they
    // are not relevant to the most recent value of `Compute`
    assert_keys! {
        db,
        TriangularQuery => (0, 1, 2, 3, 4, 5),
        FibonacciQuery => (0, 1, 2, 3, 4, 5),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }

    db.sweep_all(SweepStrategy::discard_outdated());

    // Now we just have `Triangular` and not `Fibonacci`
    assert_keys! {
        db,
        TriangularQuery => (0, 1, 2, 3, 4, 5),
        FibonacciQuery => (),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }

    // Now run `compute` *again* in next revision.
    db.salsa_runtime_mut().synthetic_write(Durability::LOW);
    assert_eq!(db.compute(5), 15);
    db.sweep_all(SweepStrategy::discard_outdated());

    // We keep triangular, but just the outermost one.
    assert_keys! {
        db,
        TriangularQuery => (5),
        FibonacciQuery => (),
        ComputeQuery => (5),
        UseTriangularQuery => (5),
        MinQuery => (),
        MaxQuery => (),
    }
}

/// Test a query with multiple layers of keys.
#[test]
fn compute_all() {
    let mut db = db::DatabaseImpl::default();

    for i in 0..6 {
        db.set_use_triangular(i, (i % 2) != 0);
    }

    db.set_min(0);
    db.set_max(6);

    db.compute_all();
    db.salsa_runtime_mut().synthetic_write(Durability::LOW);
    db.compute_all();
    db.sweep_all(SweepStrategy::discard_outdated());

    assert_keys! {
        db,
        TriangularQuery => (1, 3, 5),
        FibonacciQuery => (0, 2, 4),
        ComputeQuery => (0, 1, 2, 3, 4, 5),
        ComputeAllQuery => (()),
        UseTriangularQuery => (0, 1, 2, 3, 4, 5),
        MinQuery => (()),
        MaxQuery => (()),
    }

    // Reduce the range to exclude index 5.
    db.set_max(5);
    db.compute_all();

    assert_keys! {
        db,
        TriangularQuery => (1, 3, 5),
        FibonacciQuery => (0, 2, 4),
        ComputeQuery => (0, 1, 2, 3, 4, 5),
        ComputeAllQuery => (()),
        UseTriangularQuery => (0, 1, 2, 3, 4, 5),
        MinQuery => (()),
        MaxQuery => (()),
    }

    db.sweep_all(SweepStrategy::discard_outdated());

    // We no longer used `Compute(5)` and `Triangular(5)`; note that
    // `UseTriangularQuery(5)` is not collected, as it is an input.
    assert_keys! {
        db,
        TriangularQuery => (1, 3),
        FibonacciQuery => (0, 2, 4),
        ComputeQuery => (0, 1, 2, 3, 4),
        ComputeAllQuery => (()),
        UseTriangularQuery => (0, 1, 2, 3, 4, 5),
        MinQuery => (()),
        MaxQuery => (()),
    }
}
