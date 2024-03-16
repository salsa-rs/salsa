//! Test input deletion

use salsa::DebugWithDb;
use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, final_result);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn final_result(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result({:?})", input));
    input.field(db) + 1
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
            salsa::EventKind::WillDiscardStaleOutput { .. }
            | salsa::EventKind::DidDiscard { .. } => {
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
fn basic() {
    let mut db = Database::default();

    // Creates 3 tracked structs
    let input = MyInput::new(&db, 3);
    assert_eq!(final_result(&db, input), 4);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
        ]"#]]);

    let result = input.delete(&db);
    assert_eq!(result, Some((3,)));
    db.assert_logs(expect![[r#"
        [
            "salsa_event(DidDiscard { key: field(0) })",
            "salsa_event(DidDiscard { key: final_result(0) })",
        ]"#]]);
}
