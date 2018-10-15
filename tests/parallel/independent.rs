use crate::setup::{Input, ParDatabase, ParDatabaseImpl};
use salsa::{Database, ParallelDatabase};

/// Test two `sum` queries (on distinct keys) executing in different
/// threads. Really just a test that `fork` etc compiles.
#[test]
fn in_par_two_independent_queries() {
    let db1 = ParDatabaseImpl::default();
    let db2 = db1.fork();

    db1.query(Input).set('a', 100);
    db1.query(Input).set('b', 010);
    db1.query(Input).set('c', 001);
    db1.query(Input).set('d', 200);
    db1.query(Input).set('e', 020);
    db1.query(Input).set('f', 002);

    let thread1 = std::thread::spawn(move || db1.sum("abc"));

    let thread2 = std::thread::spawn(move || db2.sum("def"));

    assert_eq!(thread1.join().unwrap(), 111);
    assert_eq!(thread2.join().unwrap(), 222);
}
