//! Basic deletion test:
//!
//! * entities not created in a revision are deleted, as is any memoized data keyed on them.

use salsa_2022_tests::{HasLogger, Logger};

use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input(singleton)]
struct MyInput {
    field: u32,
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn basic() {
    let mut db = Database::default();
    let input1 = MyInput::new(&mut db, 3);
    let input2 = MyInput::get(&db);

    assert_eq!(input1, input2);

    let input3 = MyInput::try_get(&db);
    assert_eq!(Some(input1), input3);

    let input4 = MyInput::new(&mut db, 3);

    assert_eq!(input2, input4)
}
