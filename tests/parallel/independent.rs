use crate::setup::{Input, ParDatabase, ParDatabaseImpl};
use salsa::{Database, ParallelDatabase};

/// Test two `sum` queries (on distinct keys) executing in different
/// threads. Really just a test that `snapshot` etc compiles.
#[test]
fn in_par_two_independent_queries() {
    let db = ParDatabaseImpl::default();

    db.query(Input).set('a', 100);
    db.query(Input).set('b', 010);
    db.query(Input).set('c', 001);
    db.query(Input).set('d', 200);
    db.query(Input).set('e', 020);
    db.query(Input).set('f', 002);

    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || db.sum("abc")
    });

    let thread2 = std::thread::spawn({
        let db = db.snapshot();
        move || db.sum("def")
    });;

    assert_eq!(thread1.join().unwrap(), 111);
    assert_eq!(thread2.join().unwrap(), 222);
}
