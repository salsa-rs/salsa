#![cfg(all(feature = "inventory", feature = "salsa_unstable"))]

//! Tests for the SIEVE eviction policy.

mod common;
use common::LogDatabase;

use salsa::Database as _;
use test_log::test;

#[salsa::input]
struct MyInput {
    #[returns(copy)]
    field: u32,
}

#[salsa::tracked]
impl MyInput {
    #[salsa::tracked(returns(copy), eviction(policy = sieve, capacity = 2))]
    fn read(self, db: &dyn LogDatabase) -> u32 {
        db.push_log(format!("read({})", self.field(db)));
        self.field(db)
    }
}

#[test]
fn sieve_wraps_to_the_oldest_visited_entry() {
    let mut db = common::LoggerDatabase::default();
    let [a, b, c] = [0, 1, 2].map(|field| MyInput::new(&db, field));
    a.read(&db);
    b.read(&db);
    b.read(&db);
    a.read(&db);
    c.read(&db);
    db.assert_logs_len(3);

    // Both entries get a second chance. The hand wraps to A before C joins the queue.
    db.trigger_eviction();
    b.read(&db);
    db.assert_logs_len(0);
    a.read(&db);
    db.assert_logs_len(1);
}

#[test]
fn sieve_skips_stale_pending_evictions() {
    let mut db = common::LoggerDatabase::default();
    let [a, b, c] = [0, 1, 2].map(|field| MyInput::new(&db, field));
    a.read(&db);
    b.read(&db);
    c.read(&db);

    // C displaced A. Reading A before the sweep re-admits it and displaces B.
    a.read(&db);
    db.assert_logs_len(3);

    db.synthetic_write(salsa::Durability::HIGH);
    a.read(&db);
    db.assert_logs_len(0);
    b.read(&db);
    db.assert_logs_len(1);
}
