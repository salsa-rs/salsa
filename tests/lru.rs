#![cfg(feature = "inventory")]

//! Test that a `tracked` fn with lru options
//! compiles and executes successfully.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    #[returns(copy)]
    field: u32,
}

#[salsa::tracked(returns(clone), lru = 8)]
fn get_hot_potato(db: &dyn LogDatabase, input: MyInput) -> Arc<HotPotato> {
    db.push_log(format!("get_hot_potato({:?})", input.field(db)));
    Arc::new(HotPotato::new(input.field(db)))
}

#[salsa::tracked(returns(copy))]
fn get_hot_potato2(db: &dyn LogDatabase, input: MyInput) -> u32 {
    db.push_log(format!("get_hot_potato2({:?})", input.field(db)));
    get_hot_potato(db, input).0
}

fn load_n_potatoes() -> usize {
    N_POTATOES.with(|n| n.load(Ordering::SeqCst))
}

#[test]
fn lru_works() {
    let mut db = common::LoggerDatabase::default();
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..32u32 {
        let input = MyInput::new(&db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 8);
}

#[test]
fn lru_can_be_changed_at_runtime() {
    let mut db = common::LoggerDatabase::default();
    assert_eq!(load_n_potatoes(), 0);

    let inputs: Vec<(u32, MyInput)> = (0..32).map(|i| (i, MyInput::new(&db, i))).collect();

    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 8);

    get_hot_potato::set_lru_capacity(&mut db, 16);
    assert_eq!(load_n_potatoes(), 8);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 16);

    // Special case: setting capacity to zero disables LRU
    get_hot_potato::set_lru_capacity(&mut db, 0);
    assert_eq!(load_n_potatoes(), 16);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 32);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_keeps_dependency_info() {
    let mut db = common::LoggerDatabase::default();
    let capacity = 8;

    // Invoke `get_hot_potato2` 33 times. This will (in turn) invoke
    // `get_hot_potato`, which will trigger LRU after 8 executions.
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

#[test]
fn eviction_defaults_to_lru() {
    #[salsa::tracked(returns(copy), eviction(capacity = 1))]
    fn read(db: &dyn LogDatabase, input: MyInput) -> u32 {
        db.push_log(format!("read({})", input.field(db)));
        input.field(db)
    }

    let mut db = common::LoggerDatabase::default();
    let [a, b, c] = [0, 1, 2].map(|field| MyInput::new(&db, field));
    read::set_eviction_capacity(&mut db, 2);
    read(&db, a);
    read(&db, b);
    read(&db, b);
    read(&db, a);
    read(&db, c);
    db.assert_logs_len(3);

    // Both A and B were revisited. LRU keeps A because it was read last.
    db.trigger_lru_eviction();
    read(&db, a);
    db.assert_logs_len(0);
    read(&db, b);
    db.assert_logs_len(1);
}

#[cfg(feature = "accumulator")]
#[test]
fn accumulated_reinstates_evicted_dependency_in_lru() {
    use salsa::Accumulator as _;

    #[salsa::accumulator]
    struct Diagnostic(u32);

    #[salsa::tracked(lru = 1)]
    fn child(db: &dyn LogDatabase, input: MyInput) {
        db.push_log(format!("child({})", input.field(db)));
        Diagnostic(input.field(db)).accumulate(db);
    }

    #[salsa::tracked]
    fn parent(db: &dyn LogDatabase, input: MyInput) {
        child(db, input);
    }

    let mut db = common::LoggerDatabase::default();
    let a = MyInput::new(&db, 0);
    let b = MyInput::new(&db, 1);
    parent(&db, a);
    child(&db, b);
    db.trigger_lru_eviction();

    // Reading A's diagnostics recomputes child(A), which should displace child(B).
    assert_eq!(parent::accumulated::<Diagnostic>(&db, a)[0].0, 0);
    db.assert_logs_len(3);
    db.trigger_lru_eviction();
    child(&db, a);
    db.assert_logs_len(0);
    child(&db, b);
    db.assert_logs_len(1);
}
