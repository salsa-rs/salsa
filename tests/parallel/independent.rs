use crate::setup::{InputQuery, ParDatabase, ParDatabaseImpl};
use salsa::{Database, ParallelDatabase};

/// Test two `sum` queries (on distinct keys) executing in different
/// threads. Really just a test that `snapshot` etc compiles.
#[test]
fn in_par_two_independent_queries() {
    let mut db = ParDatabaseImpl::default();

    db.query_mut(InputQuery).set('a', 100);
    db.query_mut(InputQuery).set('b', 010);
    db.query_mut(InputQuery).set('c', 001);
    db.query_mut(InputQuery).set('d', 200);
    db.query_mut(InputQuery).set('e', 020);
    db.query_mut(InputQuery).set('f', 002);

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
