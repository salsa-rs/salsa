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
    field: u32,
}

#[salsa::tracked(lru = 8)]
fn get_hot_potato(db: &dyn LogDatabase, input: MyInput) -> Arc<HotPotato> {
    db.push_log(format!("get_hot_potato({:?})", input.field(db)));
    Arc::new(HotPotato::new(input.field(db)))
}

#[salsa::tracked]
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
    // The first collection epoch gives newly admitted values grace.
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 32);

    // Growing the resident set by 50% advances another collection epoch and
    // marks the original cohort cold, but does not evict it yet.
    for i in 32..48u32 {
        let input = MyInput::new(&db, i);
        get_hot_potato(&db, input);
    }
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 48);

    // Another 50% growth gives the original cohort its second cold inspection.
    for i in 48..72u32 {
        let input = MyInput::new(&db, i);
        get_hot_potato(&db, input);
    }
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 40);
}

#[test]
fn lru_growth_floor_can_be_changed_at_runtime() {
    let mut db = common::LoggerDatabase::default();
    assert_eq!(load_n_potatoes(), 0);

    let inputs: Vec<(u32, MyInput)> = (0..32).map(|i| (i, MyInput::new(&db, i))).collect();

    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // The first collection gives the cohort grace.
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 32);

    // Lowering the growth floor forces two more collection epochs. The first
    // marks the cohort cold and the second evicts it.
    get_hot_potato::set_lru_capacity(&mut db, 1);
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 32);

    get_hot_potato::set_lru_capacity(&mut db, 1);
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 0);

    // Setting the value to zero still disables eviction.
    get_hot_potato::set_lru_capacity(&mut db, 0);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 32);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_keeps_dependency_info() {
    let mut db = common::LoggerDatabase::default();
    let activation_floor = 8;

    // Invoke `get_hot_potato2` enough times to cross the collection floor.
    let inputs: Vec<MyInput> = (0..(activation_floor + 1))
        .map(|i| MyInput::new(&db, i as u32))
        .collect();

    for (i, input) in inputs.iter().enumerate() {
        let x = get_hot_potato2(&db, *input);
        assert_eq!(x as usize, i);
    }

    // Advance enough collection epochs to evict the inner memo values.
    db.synthetic_write(salsa::Durability::HIGH);
    get_hot_potato::set_lru_capacity(&mut db, 1);
    db.synthetic_write(salsa::Durability::HIGH);
    get_hot_potato::set_lru_capacity(&mut db, 1);
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 0);

    // We want to test that calls to `get_hot_potato2` are still considered
    // clean. Check that no new executions occur as we go here.
    db.clear_logs();

    // Calling `get_hot_potato2(0)` has to check that `get_hot_potato(0)` is still valid;
    // even though we've evicted its value, we find that it is still good.
    let p = get_hot_potato2(&db, *inputs.first().unwrap());
    assert_eq!(p, 0);
    db.assert_logs_len(0);
}
