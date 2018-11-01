use crate::setup::{Input, ParDatabase, ParDatabaseImpl};
use salsa::{Database, ParallelDatabase};

/// Test where a read and a set are racing with one another.
/// Should be atomic.
#[test]
fn in_par_get_set_race() {
    let mut db = ParDatabaseImpl::default();

    db.query_mut(Input).set('a', 100);
    db.query_mut(Input).set('b', 010);
    db.query_mut(Input).set('c', 001);

    let thread1 = std::thread::spawn({
        let db = db.snapshot();
        move || {
            let v = db.sum("abc");
            v
        }
    });

    let thread2 = std::thread::spawn(move || {
        db.query_mut(Input).set('a', 1000);
        db.sum("a")
    });

    // If the 1st thread runs first, you get 111, otherwise you get
    // 1011.
    let value1 = thread1.join().unwrap();
    assert!(value1 == 111 || value1 == 1011, "illegal result {}", value1);

    assert_eq!(thread2.join().unwrap(), 1000);
}
