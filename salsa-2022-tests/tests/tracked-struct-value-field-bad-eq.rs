//! Test a field whose `PartialEq` impl is always true.
//! This can result in us getting different results than
//! if we were to execute from scratch.

use expect_test::expect;
use salsa::DebugWithDb;
use salsa_2022_tests::{HasLogger, Logger};
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(
    MyInput,
    MyTracked<'_>,
    the_fn,
    make_tracked_struct,
    read_tracked_struct,
);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input]
struct MyInput {
    field: bool,
}

#[allow(clippy::derived_hash_with_manual_eq)]
#[derive(Eq, Hash, Debug, Clone)]
struct BadEq {
    field: bool,
}

impl PartialEq for BadEq {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl From<bool> for BadEq {
    fn from(value: bool) -> Self {
        Self { field: value }
    }
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: BadEq,
}

#[salsa::tracked]
fn the_fn(db: &dyn Db, input: MyInput) -> bool {
    let tracked = make_tracked_struct(db, input);
    read_tracked_struct(db, tracked)
}

#[salsa::tracked]
fn make_tracked_struct<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
    MyTracked::new(db, BadEq::from(input.field(db)))
}

#[salsa::tracked]
fn read_tracked_struct<'db>(db: &'db dyn Db, tracked: MyTracked<'db>) -> bool {
    tracked.field(db).field
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        match event.kind {
            salsa::EventKind::WillExecute { .. }
            | salsa::EventKind::DidValidateMemoizedValue { .. } => {
                self.push_log(format!("salsa_event({:?})", event.kind.debug(self)));
            }
            _ => {}
        }
    }
}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn execute() {
    let mut db = Database::default();

    let input = MyInput::new(&db, true);
    let result = the_fn(&db, input);
    assert!(result);

    db.assert_logs(expect![[r#"
        [
            "salsa_event(WillExecute { database_key: the_fn(0) })",
            "salsa_event(WillExecute { database_key: make_tracked_struct(0) })",
            "salsa_event(WillExecute { database_key: read_tracked_struct(0) })",
        ]"#]]);

    // Update the input to `false` and re-execute.
    input.set_field(&mut db).to(false);
    let result = the_fn(&db, input);

    // If the `Eq` impl were working properly, we would
    // now return `false`. But because the `Eq` is considered
    // equal we re-use memoized results and so we get true.
    assert!(result);

    db.assert_logs(expect![[r#"
        [
            "salsa_event(WillExecute { database_key: make_tracked_struct(0) })",
            "salsa_event(DidValidateMemoizedValue { database_key: read_tracked_struct(0) })",
            "salsa_event(DidValidateMemoizedValue { database_key: the_fn(0) })",
        ]"#]]);
}
