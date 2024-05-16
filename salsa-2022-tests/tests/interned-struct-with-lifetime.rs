//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
use salsa::DebugWithDb;
use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(InternedString<'_>, InternedPair<'_>, intern_stuff);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::interned]
struct InternedString<'db> {
    data: String,
}

#[salsa::interned]
struct InternedPair<'db> {
    data: (InternedString<'db>, InternedString<'db>),
}

#[salsa::tracked]
fn intern_stuff(db: &dyn Db) -> String {
    let s1 = InternedString::new(db, format!("Hello, "));
    let s2 = InternedString::new(db, format!("World, "));
    let s3 = InternedPair::new(db, (s1, s2));
    format!("{:?}", s3.debug(db))
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

    expect![[r#"
        "InternedPair { [salsa id]: 0, data: (InternedString { [salsa id]: 0, data: \"Hello, \" }, InternedString { [salsa id]: 1, data: \"World, \" }) }"
    "#]].assert_debug_eq(&intern_stuff(&db));
    db.assert_logs(expect!["[]"]);
}
