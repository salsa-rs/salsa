//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.

mod common;
use common::{HasLogger, Logger};

use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input]
struct MyInput {
    field: String,
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
fn execute() {
    let mut db = Database::default();

    let input = MyInput::new(&db, "Hello".to_string());

    // Overwrite field with an empty String
    // and store the old value in my_string
    let mut my_string = input.set_field(&mut db).to(String::new());
    my_string.push_str(" World!");

    // Set the field back to out initial String,
    // expecting to get the empty one back
    assert_eq!(input.set_field(&mut db).to(my_string), "");

    // Check if the stored String is the one we expected
    assert_eq!(input.field(&db), "Hello World!");
}
