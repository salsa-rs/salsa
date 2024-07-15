//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use common::{HasLogger, Logger};

use expect_test::expect;
use salsa::Setter;
use test_log::test;

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn final_result(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result({:?})", input));
    intermediate_result(db, input).field(db) * 2
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn intermediate_result(db: &dyn Db, input: MyInput) -> MyTracked<'_> {
    db.push_log(format!("intermediate_result({:?})", input));
    MyTracked::new(db, input.field(db) / 2)
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn execute() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 22);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput { [salsa id]: 0 })",
            "intermediate_result(MyInput { [salsa id]: 0 })",
        ]"#]]);

    // Intermediate result is the same, so final result does
    // not need to be recomputed:
    input.set_field(&mut db).to(23);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput { [salsa id]: 0 })",
        ]"#]]);

    input.set_field(&mut db).to(24);
    assert_eq!(final_result(&db, input), 24);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput { [salsa id]: 0 })",
            "final_result(MyInput { [salsa id]: 0 })",
        ]"#]]);
}
