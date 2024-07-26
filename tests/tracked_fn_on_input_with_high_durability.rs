//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
#![allow(warnings)]

use expect_test::expect;

use common::{HasLogger, Logger};
use salsa::plumbing::HasStorage;
use salsa::{Database, Durability, Event, EventKind, Setter};

mod common;
#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[test]
fn execute() {
    #[salsa::db]
    #[derive(Default)]
    struct Database {
        storage: salsa::Storage<Self>,
        logger: Logger,
    }

    #[salsa::db]
    impl salsa::Database for Database {
        fn salsa_event(&self, event: Event) {
            match event.kind {
                EventKind::WillCheckCancellation => {}
                _ => {
                    self.push_log(format!("salsa_event({:?})", event.kind));
                }
            }
        }
    }

    impl HasLogger for Database {
        fn logger(&self) -> &Logger {
            &self.logger
        }
    }

    let mut db = Database::default();
    let input_low = MyInput::new(&db, 22);
    let input_high = MyInput::builder(&db).durability(Durability::HIGH).new(2200);

    assert_eq!(tracked_fn(&db, input_low), 44);
    assert_eq!(tracked_fn(&db, input_high), 4400);

    db.assert_logs(expect![[r#"
        [
            "salsa_event(WillExecute { database_key: tracked_fn(0) })",
            "salsa_event(WillExecute { database_key: tracked_fn(1) })",
        ]"#]]);

    db.synthetic_write(Durability::LOW);

    assert_eq!(tracked_fn(&db, input_low), 44);
    assert_eq!(tracked_fn(&db, input_high), 4400);

    // There's currently no good way to verify whether an input was validated using shallow or deep comparison.
    // All we can do for now is verify that the values were validated.
    // Note: It maybe confusing why it validates `input_high` when the write has `Durability::LOW`.
    // This is because all values must be validated whenever a write occurs. It doesn't mean that it
    // executed the query.
    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidValidateMemoizedValue { database_key: tracked_fn(0) })",
            "salsa_event(DidValidateMemoizedValue { database_key: tracked_fn(1) })",
        ]"#]]);
}
