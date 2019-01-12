use crate::db;
use crate::group::{FibonacciQuery, GcDatabase};
use salsa::debug::DebugQueryTable;
use salsa::{Database, SweepStrategy};

#[test]
fn sweep_default() {
    let db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = db.query(FibonacciQuery).keys();
    assert_eq!(k.len(), 6);

    db.salsa_runtime().next_revision();

    db.fibonacci(5);
    db.fibonacci(3);

    // fibonacci is a constant, so it will not be invalidated,
    // hence we keep 3 and 5 but remove the rest.
    db.sweep_all(SweepStrategy::default());
    let mut k: Vec<_> = db.query(FibonacciQuery).keys();
    k.sort();
    assert_eq!(k, vec![3, 5]);

    // Even though we ran the sweep, 5 is still in cache
    db.clear_log();
    db.fibonacci(5);
    db.assert_log(&[]);

    // Same but we discard values this time.
    db.sweep_all(SweepStrategy::default().discard_values());
    let mut k: Vec<_> = db.query(FibonacciQuery).keys();
    k.sort();
    assert_eq!(k, vec![3, 5]);

    // Now we have to recompute
    db.clear_log();
    db.fibonacci(5);
    db.assert_log(&[
        "fibonacci(5)",
        "fibonacci(4)",
        "fibonacci(3)",
        "fibonacci(2)",
        "fibonacci(1)",
        "fibonacci(0)",
    ]);
}
