#![cfg(feature = "inventory")]

//! Test that a `tracked` fn with lru options
//! compiles and executes successfully.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod common;
use common::LogDatabase;

use salsa::{Database as _, DropChannelReceiver};
use test_log::test;

#[derive(Debug)]
struct HotPotato(u32, Arc<AtomicUsize>);

impl Eq for HotPotato {}
impl PartialEq for HotPotato {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

thread_local! {
    static N_POTATOES: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0))
}

impl HotPotato {
    fn new(id: u32) -> HotPotato {
        N_POTATOES.with(|n| {
            n.fetch_add(1, Ordering::SeqCst);
            HotPotato(id, n.clone())
        })
    }
}

impl Drop for HotPotato {
    fn drop(&mut self) {
        self.1.fetch_sub(1, Ordering::SeqCst);
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

fn wait_until_n_potatoes(n: usize) {
    let now = std::time::Instant::now();
    while load_n_potatoes() != n {
        std::thread::yield_now();
        if now.elapsed().as_secs() > 10 {
            panic!(
                "timed out waiting for {} potatoes, we've got {} instead",
                n,
                load_n_potatoes()
            );
        }
    }
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
    wait_until_n_potatoes(8);
    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_can_be_changed_at_runtime_sync() {
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
    std::thread::sleep(std::time::Duration::from_millis(100));

    wait_until_n_potatoes(8);

    get_hot_potato::set_lru_capacity(&mut db, 16);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    wait_until_n_potatoes(16);

    // Special case: setting capacity to zero disables LRU
    get_hot_potato::set_lru_capacity(&mut db, 0);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    wait_until_n_potatoes(32);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
}

#[test]
fn lru_keeps_dependency_info_sync() {
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
fn lru_works_async() {
    let (mut db, drop_chan) = common::LoggerDatabase::new_with_drop_channel();
    let drop_thread = drop_thread(drop_chan);
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..32u32 {
        let input = MyInput::new(&db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    wait_until_n_potatoes(8);
    drop(db);
    wait_until_n_potatoes(0);
    drop_thread.join().unwrap();
}

#[test]
fn lru_can_be_changed_at_runtime() {
    let (mut db, drop_chan) = common::LoggerDatabase::new_with_drop_channel();
    let drop_thread = drop_thread(drop_chan);
    assert_eq!(load_n_potatoes(), 0);

    let inputs: Vec<(u32, MyInput)> = (0..32).map(|i| (i, MyInput::new(&db, i))).collect();

    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    std::thread::sleep(std::time::Duration::from_millis(100));

    wait_until_n_potatoes(8);

    get_hot_potato::set_lru_capacity(&mut db, 16);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    wait_until_n_potatoes(16);

    // Special case: setting capacity to zero disables LRU
    get_hot_potato::set_lru_capacity(&mut db, 0);
    for &(i, input) in inputs.iter() {
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i);
    }

    assert_eq!(load_n_potatoes(), 32);
    // trigger the GC
    db.synthetic_write(salsa::Durability::HIGH);
    wait_until_n_potatoes(32);

    drop(db);
    assert_eq!(load_n_potatoes(), 0);
    drop_thread.join().unwrap();
}

#[test]
fn lru_keeps_dependency_info() {
    let (mut db, drop_chan) = common::LoggerDatabase::new_with_drop_channel();
    let drop_thread = drop_thread(drop_chan);
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
    drop(db);
    drop_thread.join().unwrap();
}

#[cfg(feature = "shuttle")]
fn drop_thread(receiver: DropChannelReceiver) -> shuttle::thread::JoinHandle<()> {
    shuttle::thread::spawn(|| receiver.into_iter().for_each(|_| ()))
}

#[cfg(not(feature = "shuttle"))]
fn drop_thread(receiver: DropChannelReceiver) -> std::thread::JoinHandle<()> {
    std::thread::spawn(|| receiver.into_iter().for_each(|_| ()))
}
