//! Test setting LRU actually limits the number of things in the database;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use salsa::Database as _;

#[derive(Debug, PartialEq, Eq)]
struct HotPotato(u32);

static N_POTATOES: AtomicUsize = AtomicUsize::new(0);

impl HotPotato {
    fn new(id: u32) -> HotPotato {
        N_POTATOES.fetch_add(1, Ordering::SeqCst);
        HotPotato(id)
    }
}

impl Drop for HotPotato {
    fn drop(&mut self) {
        N_POTATOES.fetch_sub(1, Ordering::SeqCst);
    }
}

#[salsa::query_group(QueryGroupStorage)]
trait QueryGroup {
    fn get(&self, x: u32) -> Arc<HotPotato>;
}

fn get(_db: &impl QueryGroup, x: u32) -> Arc<HotPotato> {
    Arc::new(HotPotato::new(x))
}

#[salsa::database(QueryGroupStorage)]
#[derive(Default)]
struct Database {
    runtime: salsa::Runtime<Database>,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime<Database> {
        &self.runtime
    }
}

#[test]
fn lru_works() {
    let mut db = Database::default();
    let cap = 32;
    db.query_mut(GetQuery).set_lru_capacity(32);
    assert_eq!(N_POTATOES.load(Ordering::SeqCst), 0);

    for i in 0..128u32 {
        let p = db.get(i);
        assert_eq!(p.0, i)
    }
    assert_eq!(N_POTATOES.load(Ordering::SeqCst), cap);

    for i in 0..128u32 {
        let p = db.get(i);
        assert_eq!(p.0, i)
    }
    assert_eq!(N_POTATOES.load(Ordering::SeqCst), cap);
    drop(db);
    assert_eq!(N_POTATOES.load(Ordering::SeqCst), 0);
}
