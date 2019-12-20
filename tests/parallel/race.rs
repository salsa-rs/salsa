use crate::setup::{ParDatabase, ParDatabaseImpl, ParDatabaseMut};
use salsa::ParallelDatabase;

/// Test where a read and a set are racing with one another.
/// Should be atomic.
#[test]
fn in_par_get_set_race() {
    let mut db = ParDatabaseImpl::default();

    db.set_input('a', 100);
    db.set_input('b', 010);
    db.set_input('c', 001);

    let thread1 = std::thread::spawn({
        let mut db = db.snapshot();
        move || {
            let v = db.sum("abc");
            v
        }
    });

    let thread2 = std::thread::spawn(move || {
        db.set_input('a', 1000);
        db.sum("a")
    });

    // If the 1st thread runs first, you get 111, otherwise you get
    // 1011; if they run concurrently and the 1st thread observes the
    // cancelation, you get back usize::max.
    let value1 = thread1.join().unwrap();
    assert!(
        value1 == 111 || value1 == 1011 || value1 == std::usize::MAX,
        "illegal result {}",
        value1
    );

    assert_eq!(thread2.join().unwrap(), 1000);
}
