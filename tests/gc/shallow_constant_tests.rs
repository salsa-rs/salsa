use crate::db;
use crate::group::{FibonacciQuery, GcDatabase};
use salsa::debug::DebugQueryTable;
use salsa::{Database, Durability, SweepStrategy};

// For constant values (like `fibonacci`), we only keep the values
// that were used in the latest revision, not the sub-values that
// they required to be computed.

#[test]
fn one_rev() {
    let db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = FibonacciQuery.in_db(&db).entries();
    assert_eq!(k.len(), 6);

    // Everything was used in this revision, so
    // nothing gets collected.
    db.sweep_all(SweepStrategy::discard_outdated());
    assert_eq!(k.len(), 6);
}

#[test]
fn two_rev_nothing() {
    let mut db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = FibonacciQuery.in_db(&db).entries();
    assert_eq!(k.len(), 6);

    db.salsa_runtime_mut().synthetic_write(Durability::LOW);

    // Nothing was used in this revision, so
    // everything gets collected.
    db.sweep_all(SweepStrategy::discard_outdated());

    let k: Vec<_> = FibonacciQuery.in_db(&db).entries();
    assert_eq!(k.len(), 0);
}

#[test]
fn two_rev_one_use() {
    let mut db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = FibonacciQuery.in_db(&db).entries();
    assert_eq!(k.len(), 6);

    db.salsa_runtime_mut().synthetic_write(Durability::LOW);

    db.fibonacci(5);

    // fibonacci is a constant, so it will not be invalidated,
    // hence we keep `fibonacci(5)` but remove 0..=4.
    db.sweep_all(SweepStrategy::discard_outdated());

    assert_keys! {
        db,
        FibonacciQuery => (5),
    }
}

#[test]
fn two_rev_two_uses() {
    let mut db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = FibonacciQuery.in_db(&db).entries();
    assert_eq!(k.len(), 6);

    db.salsa_runtime_mut().synthetic_write(Durability::LOW);

    db.fibonacci(5);
    db.fibonacci(3);

    // fibonacci is a constant, so it will not be invalidated,
    // hence we keep 3 and 5 but remove the rest.
    db.sweep_all(SweepStrategy::discard_outdated());

    assert_keys! {
        db,
        FibonacciQuery => (3, 5),
    }
}
