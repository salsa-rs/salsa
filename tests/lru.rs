//! Test that a `tracked` fn with lru options
//! compiles and executes successfully.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

mod common;
use common::LogDatabase;
use salsa::Database as _;
use test_log::test;

#[derive(Debug, PartialEq, Eq)]
struct HotPotato(u32);

thread_local! {
    static N_POTATOES: AtomicUsize = const { AtomicUsize::new(0) }
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

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked(lru = 32)]
fn get_hot_potato(db: &dyn LogDatabase, input: MyInput) -> Arc<HotPotato> {
    db.push_log(format!("get_hot_potato({:?})", input.field(db)));
    Arc::new(HotPotato::new(input.field(db)))
}

#[salsa::tracked]
fn get_hot_potato2(db: &dyn LogDatabase, input: MyInput) -> u32 {
    db.push_log(format!("get_hot_potato2({:?})", input.field(db)));
    get_hot_potato(db, input).0
}

#[salsa::tracked(lru = 32)]
fn get_volatile(db: &dyn LogDatabase, _input: MyInput) -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    db.report_untracked_read();
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn load_n_potatoes() -> usize {
    N_POTATOES.with(|n| n.load(Ordering::SeqCst))
}

#[test]
fn lru_works() {
    let db = common::LoggerDatabase::default();
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..128u32 {
        let input = MyInput::new(&db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&db, 0);
    assert_eq!(load_n_potatoes(), 32);
}

#[test]
fn lru_doesnt_break_volatile_queries() {
    let db = common::LoggerDatabase::default();

    // Create all inputs first, so that there are no revision changes among calls to `get_volatile`
    let inputs: Vec<MyInput> = (0..128usize).map(|i| MyInput::new(&db, i as u32)).collect();

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
    let db = common::LoggerDatabase::default();
    assert_eq!(load_n_potatoes(), 0);

    let inputs: Vec<(u32, MyInput)> = (0..128).map(|i| (i, MyInput::new(&db, i))).collect();

    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&db, 0);
    assert_eq!(load_n_potatoes(), 32);

    get_hot_potato::set_lru_capacity(&db, 64);
    assert_eq!(load_n_potatoes(), 32);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&db, 0);
    assert_eq!(load_n_potatoes(), 64);

    // Special case: setting capacity to zero disables LRU
    get_hot_potato::set_lru_capacity(&db, 0);
    assert_eq!(load_n_potatoes(), 64);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }

    // Create a new input to change the revision, and trigger the GC
    MyInput::new(&db, 0);
    assert_eq!(load_n_potatoes(), 128);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_keeps_dependency_info() {
    let mut db = common::LoggerDatabase::default();
    let capacity = 32;

    // Invoke `get_hot_potato2` 33 times. This will (in turn) invoke
    // `get_hot_potato`, which will trigger LRU after 32 executions.
    let inputs: Vec<MyInput> = (0..(capacity + 1))
        .map(|i| MyInput::new(&db, i as u32))
        .collect();

    for (i, input) in inputs.iter().enumerate() {
        let x = get_hot_potato2(&db, *input);
        assert_eq!(x as usize, i);
    }

    db.synthetic_write(salsa::Durability::HIGH);

    // We want to test that calls to `get_hot_potato2` are still considered
    // clean. Check that no new executions occur as we go here.
    db.assert_logs_len((capacity + 1) * 2);

    // calling `get_hot_potato2(0)` has to check that `get_hot_potato(0)` is still valid;
    // even though we've evicted it (LRU), we find that it is still good
    let p = get_hot_potato2(&db, *inputs.first().unwrap());
    assert_eq!(p, 0);
    db.assert_logs_len(0);
}
