//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, get_hot_potato);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

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

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }
}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

fn load_n_potatoes() -> usize {
    N_POTATOES.with(|n| n.load(Ordering::SeqCst))
}

#[test]
fn execute() {
    let mut db = Database::default();
    assert_eq!(load_n_potatoes(), 0);

    for i in 0..128u32 {
        let input = MyInput::new(&mut db, i);
        let p = get_hot_potato(&db, input);
        assert_eq!(p.0, i)
    }
    assert_eq!(load_n_potatoes(), 32);


}