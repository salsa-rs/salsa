//! Test that a `tracked` fn with lru options
//! compiles and executes successfully.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, get_hot_potato, get_volatile);

trait Db: salsa::DbWithJar<Jar> {}

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

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(jar = Jar, lru = 32)]
fn get_hot_potato(db: &dyn Db, input: MyInput) -> Arc<HotPotato> {
    Arc::new(HotPotato::new(input.field(db)))
}

#[salsa::tracked(jar = Jar, lru = 32)]
fn get_volatile(db: &dyn Db, _input: MyInput) -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    db.salsa_runtime().report_untracked_read();
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }
}

impl Db for Database {}

fn load_n_potatoes() -> usize {
    N_POTATOES.with(|n| n.load(Ordering::SeqCst))
}

#[test]
fn lru_works() {
    let mut db = Database::default();
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..128u32 {
        let input = MyInput::new(&mut db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&mut db, 0);
    assert_eq!(load_n_potatoes(), 32);
}

#[test]
fn lru_doesnt_break_volatile_queries() {
    let mut db = Database::default();

    // Create all inputs first, so that there are no revision changes among calls to `get_volatile`
    let inputs: Vec<MyInput> = (0..128usize)
        .map(|i| MyInput::new(&mut db, i as u32))
        .collect();

    // Here, we check that we execute each volatile query at most once, despite
    // LRU. That does mean that we have more values in DB than the LRU capacity,
    // but it's much better than inconsistent results from volatile queries!
    for _ in 0..3 {
        for (i, input) in inputs.iter().enumerate() {
            let x = get_volatile(&db, *input);
            assert_eq!(x, i);
        }
    }
}

#[test]
fn lru_can_be_changed_at_runtime() {
    let mut db = Database::default();
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..128u32 {
        let input = MyInput::new(&mut db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&mut db, 0);
    assert_eq!(load_n_potatoes(), 32);

    get_hot_potato::set_lru_capacity(&db, 64);
    assert_eq!(load_n_potatoes(), 32);
    for i in 0..128u32 {
        let input = MyInput::new(&mut db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&mut db, 0);
    assert_eq!(load_n_potatoes(), 64);

    // Special case: setting capacity to zero disables LRU
    get_hot_potato::set_lru_capacity(&db, 0);
    assert_eq!(load_n_potatoes(), 64);
    for i in 0..128u32 {
        let input = MyInput::new(&mut db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&mut db, 0);
    assert_eq!(load_n_potatoes(), 192);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}
