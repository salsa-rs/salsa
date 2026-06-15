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
    // Probation values age without requiring any additional admissions. The
    // maintenance budget spreads inspection of the due cohort across revisions.
    for _ in 1..16 {
        db.synthetic_write(salsa::Durability::HIGH);
    }
    assert_eq!(load_n_potatoes(), 32);

    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 24);

    for _ in 0..3 {
        db.synthetic_write(salsa::Durability::HIGH);
    }
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_maintenance_budget_can_be_changed_at_runtime() {
    let mut db = common::LoggerDatabase::default();
    assert_eq!(load_n_potatoes(), 0);

    let inputs: Vec<(u32, MyInput)> = (0..32).map(|i| (i, MyInput::new(&db, i))).collect();

    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // Raising the maintenance floor lets the probation cohort be inspected in
    // one revision once its grace period expires.
    get_hot_potato::set_lru_capacity(&mut db, 32);
    for _ in 1..16 {
        db.synthetic_write(salsa::Durability::HIGH);
    }
    assert_eq!(load_n_potatoes(), 32);
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
    let input_count = 9;

    let inputs: Vec<MyInput> = (0..input_count)
        .map(|i| MyInput::new(&db, i as u32))
        .collect();

    for (i, input) in inputs.iter().enumerate() {
        let x = get_hot_potato2(&db, *input);
        assert_eq!(x as usize, i);
    }

    // Advance enough revisions to evict the inner memo values. Use a maintenance
    // budget large enough to inspect the entire cohort on each due revision.
    get_hot_potato::set_lru_capacity(&mut db, input_count);
    for _ in 0..16 {
        db.synthetic_write(salsa::Durability::HIGH);
    }
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
