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

/// The by-name route exists because the generated `set_lru_capacity` is private
/// to the declaring module, and for an *associated* tracked fn the macro emits it
/// inside the body of the outer function, where nothing can reach it. Callers
/// that need to retune such a query have only the ingredient registry.
///
/// So it is not enough for the call to return a plausible number: it has to move
/// the same memos the direct setter moves. That is what the potato count checks.
#[test]
fn lru_capacity_can_be_set_by_name() {
    let mut db = common::LoggerDatabase::default();
    let inputs: Vec<(u32, MyInput)> = (0..32).map(|i| (i, MyInput::new(&db, i))).collect();

    for &(i, input) in inputs.iter() {
        assert_eq!(get_hot_potato(&db, input).0, i);
    }
    assert_eq!(load_n_potatoes(), 32);
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 8, "the declared `lru = 8` should hold first");

    // Positive control: retuning by name has to change what survives eviction.
    assert_eq!(db.set_lru_capacity_by_name("get_hot_potato", 16), 1);
    for &(i, input) in inputs.iter() {
        assert_eq!(get_hot_potato(&db, input).0, i);
    }
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 16);

    // And the floor is 1, not 0 — 0 disables eviction entirely (see the
    // `lru_can_be_changed_at_runtime` case above), which is the trap for anyone
    // reaching for "0" to mean "keep nothing".
    assert_eq!(db.set_lru_capacity_by_name("get_hot_potato", 1), 1);
    for &(i, input) in inputs.iter() {
        assert_eq!(get_hot_potato(&db, input).0, i);
    }
    db.synthetic_write(salsa::Durability::HIGH);
    assert_eq!(load_n_potatoes(), 1);
}

/// A name that matches nothing must not read as success. Two ways to miss: a
/// typo (or a query upstream renamed), and a real query that carries no `lru` at
/// all. Both answer zero, because in both cases the requested capacity went
/// nowhere — silently returning "fine" is exactly how a knob rots into a no-op.
#[test]
fn missing_and_uncapped_queries_report_zero() {
    let mut db = common::LoggerDatabase::default();

    // Warm both queries up: ingredients enter the registry on first use, and an
    // empty registry would make every assertion below pass for the wrong reason.
    let input = MyInput::new(&db, 1);
    let _ = get_hot_potato(&db, input);
    let _ = get_hot_potato2(&db, input);

    assert_eq!(db.set_lru_capacity_by_name("no_such_query", 4), 0);
    assert_eq!(db.set_lru_capacity_by_name("get_hot_potato2", 4), 0);
    // Sanity that the zeros above are not simply "nothing is ever tunable".
    assert_eq!(db.set_lru_capacity_by_name("get_hot_potato", 4), 1);

    let names = db.lru_capacity_names();
    assert!(
        names.contains(&"get_hot_potato"),
        "tunable query missing from {names:?}"
    );
    assert!(
        !names.contains(&"get_hot_potato2"),
        "uncapped query listed as tunable in {names:?}"
    );
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
