//! Test that a constant `tracked` fn (has no inputs)
//! compiles and executes successfully.
#![allow(warnings)]

mod common;

use common::{HasLogger, Logger};
use expect_test::expect;
use salsa::{Database as _, Durability, Event, EventKind};

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn Db, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl salsa::Database for Database {
    fn salsa_event(&self, event: Event) {
        if let EventKind::WillExecute { .. } | EventKind::DidValidateMemoizedValue { .. } =
            event.kind
        {
            self.push_log(format!("{:?}", event.kind));
        }
    }
}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::db]
impl Db for Database {}

#[test]
fn execute() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input), 44);

    db.assert_logs(expect![[r#"
        [
            "WillExecute { database_key: tracked_fn(0) }",
        ]"#]]);

    // Bumps the revision
    db.synthetic_write(Durability::LOW);

    // Query should re-run
    assert_eq!(tracked_fn(&db, input), 44);

    db.assert_logs(expect![[r#"
        [
            "DidValidateMemoizedValue { database_key: tracked_fn(0) }",
        ]"#]]);
}
