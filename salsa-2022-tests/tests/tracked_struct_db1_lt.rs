//! Test that tracked structs with lifetimes not named `'db`
//! compile successfully.

use salsa_2022_tests::{HasLogger, Logger};

use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked1<'_>, MyTracked2<'_>);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
struct MyTracked1<'db1> {
    field: MyTracked2<'db1>,
}

#[salsa::tracked(jar = Jar)]
struct MyTracked2<'db2> {
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
fn create_db() {}
