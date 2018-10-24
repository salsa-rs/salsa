use crate::db;
use crate::group::{Fibonacci, GcDatabase};
use salsa::debug::DebugQueryTable;
use salsa::Database;

// For constant values (like `fibonacci`), we only keep the values
// that were used in the latest revision, not the sub-values that
// they required to be computed.

#[test]
fn one_rev() {
    let db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = db.query(Fibonacci).keys();
    assert_eq!(k.len(), 6);

    // Everything was used in this revision, so
    // nothing gets collected.
    db.sweep_all();
    assert_eq!(k.len(), 6);
}

#[test]
fn two_rev_nothing() {
    let db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = db.query(Fibonacci).keys();
    assert_eq!(k.len(), 6);

    db.salsa_runtime().next_revision();

    // Nothing was used in this revision, so
    // everything gets collected.
    db.sweep_all();

    let k: Vec<_> = db.query(Fibonacci).keys();
    assert_eq!(k.len(), 0);
}

#[test]
fn two_rev_one_use() {
    let db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = db.query(Fibonacci).keys();
    assert_eq!(k.len(), 6);

    db.salsa_runtime().next_revision();

    db.fibonacci(5);

    // fibonacci is a constant, so it will not be invalidated,
    // hence we keep `fibonacci(5)` but remove 0..=4.
    db.sweep_all();

    let k: Vec<_> = db.query(Fibonacci).keys();
    assert_eq!(k, vec![5]);
}

#[test]
fn two_rev_two_uses() {
    let db = db::DatabaseImpl::default();

    db.fibonacci(5);

    let k: Vec<_> = db.query(Fibonacci).keys();
    assert_eq!(k.len(), 6);

    db.salsa_runtime().next_revision();

    db.fibonacci(5);
    db.fibonacci(3);

    // fibonacci is a constant, so it will not be invalidated,
    // hence we keep 3 and 5 but remove the rest.
    db.sweep_all();

    let mut k: Vec<_> = db.query(Fibonacci).keys();
    k.sort();
    assert_eq!(k, vec![3, 5]);
}
