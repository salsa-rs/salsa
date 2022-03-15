//! Test setting LRU actually limits the number of things in the database;
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use salsa::{Database as _, Durability};

trait LruPeek {
    fn log(&self, event: String);
}

#[derive(Debug, PartialEq, Eq)]
struct HotPotato(u32);

thread_local! {
    static N_POTATOES: AtomicUsize = AtomicUsize::new(0)
}

impl HotPotato {
    fn new(id: u32) -> HotPotato {
        N_POTATOES.with(|n| n.fetch_add(1, Ordering::SeqCst));
        HotPotato(id)
    }
}

impl Drop for HotPotato {
    fn drop(&mut self) {
        N_POTATOES.with(|n| n.fetch_sub(1, Ordering::SeqCst));
    }
}

#[salsa::query_group(QueryGroupStorage)]
trait QueryGroup: salsa::Database + LruPeek {
    fn get2(&self, x: u32) -> u32;
    fn get(&self, x: u32) -> Arc<HotPotato>;
    fn get_volatile(&self, x: u32) -> usize;
}

/// Create a hotpotato (this will increment the counter above)
fn get(db: &dyn QueryGroup, x: u32) -> Arc<HotPotato> {
    db.log(format!("get({x})"));
    Arc::new(HotPotato::new(x))
}

/// Forward to the `get` query
fn get2(db: &dyn QueryGroup, x: u32) -> u32 {
    db.log(format!("get2({x})"));
    db.get(x).0
}

// Like `get`, but with a volatile input, which means it can't
// be LRU'd.
fn get_volatile(db: &dyn QueryGroup, _x: u32) -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    db.salsa_runtime().report_untracked_read();
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

#[salsa::database(QueryGroupStorage)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logs: RefCell<Vec<String>>,
}

impl salsa::Database for Database {}

impl LruPeek for Database {
    fn log(&self, event: String) {
        eprintln!("{event}");
        self.logs.borrow_mut().push(event);
    }
}

fn load_n_potatoes() -> usize {
    N_POTATOES.with(|n| n.load(Ordering::SeqCst))
}

#[test]
fn lru_works() {
    let mut db = Database::default();
    GetQuery.in_db_mut(&mut db).set_lru_capacity(32);
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..128u32 {
        let p = db.get(i);
        assert_eq!(p.0, i)
    }
    assert_eq!(load_n_potatoes(), 32);

    for i in 0..128u32 {
        let p = db.get(i);
        assert_eq!(p.0, i)
    }
    assert_eq!(load_n_potatoes(), 32);

    GetQuery.in_db_mut(&mut db).set_lru_capacity(32);
    assert_eq!(load_n_potatoes(), 32);

    GetQuery.in_db_mut(&mut db).set_lru_capacity(64);
    assert_eq!(load_n_potatoes(), 32);
    for i in 0..128u32 {
        let p = db.get(i);
        assert_eq!(p.0, i)
    }
    assert_eq!(load_n_potatoes(), 64);

    // Special case: setting capacity to zero disables LRU
    GetQuery.in_db_mut(&mut db).set_lru_capacity(0);
    assert_eq!(load_n_potatoes(), 64);
    for i in 0..128u32 {
        let p = db.get(i);
        assert_eq!(p.0, i)
    }
    assert_eq!(load_n_potatoes(), 128);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_doesnt_break_volatile_queries() {
    let mut db = Database::default();
    GetVolatileQuery.in_db_mut(&mut db).set_lru_capacity(32);
    // Here, we check that we execute each volatile query at most once, despite
    // LRU. That does mean that we have more values in DB than the LRU capacity,
    // but it's much better than inconsistent results from volatile queries!
    for i in (0..3).flat_map(|_| 0..128usize) {
        let x = db.get_volatile(i as u32);
        assert_eq!(x, i)
    }
}

#[test]
fn lru_keeps_dependency_info() {
    let mut db = Database::default();
    let capacity = 4;
    GetQuery.in_db_mut(&mut db).set_lru_capacity(capacity);

    // Invoke `get2` 128 times. This will (in turn) invoke
    // `get`, which will trigger LRU after 32 executions.
    for i in 0..(capacity + 1) {
        let p = db.get2(i as u32);
        assert_eq!(p, i as u32);
    }

    db.salsa_runtime_mut().synthetic_write(Durability::HIGH);

    // We want to test that calls to `get2` are still considered
    // clean. Check that no new executions occur as we go here.
    let events = db.logs.borrow().len();
    assert_eq!(events, (capacity + 1) * 2);

    // calling `get2(0)` has to check that `get(0)` is still valid;
    // even though we've evicted it (LRU), we find that it is still good
    let p = db.get2(0);
    assert_eq!(p, 0);
    assert_eq!(db.logs.borrow().len(), events);
}
