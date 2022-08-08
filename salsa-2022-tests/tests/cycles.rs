
#![allow(warnings)]

#[salsa::jar(db = Db)]
struct Jar(MyInput, memoized_a, memoized_b);

trait Db: salsa::DbWithJar<Jar> {}

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

#[salsa::input(jar = Jar)]
struct MyInput {

}

#[salsa::tracked(jar = Jar, recovery_fn = my_recover_fn)]
fn memoized_a(db: &dyn Db, input: MyInput) -> () {
    memoized_b(db, input)
}

fn my_recover_fn(db: &dyn Db, cycle: &salsa::Cycle, input: MyInput) -> () {
}

#[salsa::tracked(jar = Jar)]
fn memoized_b(db: &dyn Db, input: MyInput) -> () {
    memoized_a(db, input)
}

#[test]
fn execute() {
    let mut db = Database::default();
    let input = MyInput::new(&mut db);
    memoized_a(&db, input);
}