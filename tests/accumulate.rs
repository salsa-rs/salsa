//! Basic deletion test:
//!
//! * entities not created in a revision are deleted, as is any memoized data keyed on them.

mod common;
use common::{HasLogger, Logger};

use expect_test::expect;
use salsa::{Accumulator, Setter};
use test_log::test;

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::input]
struct MyInput {
    field_a: u32,
    field_b: u32,
}

#[salsa::accumulator]
#[derive(Clone, Debug)]
struct Logs(#[allow(dead_code)] String);

#[salsa::tracked]
fn push_logs(db: &dyn Db, input: MyInput) {
    db.push_log(format!(
        "push_logs(a = {}, b = {})",
        input.field_a(db),
        input.field_b(db)
    ));

    // We don't invoke `push_a_logs` (or `push_b_logs`) with a value of 1 or less.
    // This allows us to test what happens a change in inputs causes a function not to be called at all.
    if input.field_a(db) > 1 {
        push_a_logs(db, input);
    }

    if input.field_b(db) > 1 {
        push_b_logs(db, input);
    }
}

#[salsa::tracked]
fn push_a_logs(db: &dyn Db, input: MyInput) {
    let field_a = input.field_a(db);
    db.push_log(format!("push_a_logs({})", field_a));

    for i in 0..field_a {
        Logs(format!("log_a({} of {})", i, field_a)).accumulate(db);
    }
}

#[salsa::tracked]
fn push_b_logs(db: &dyn Db, input: MyInput) {
    let field_a = input.field_b(db);
    db.push_log(format!("push_b_logs({})", field_a));

    for i in 0..field_a {
        Logs(format!("log_b({} of {})", i, field_a)).accumulate(db);
    }
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl salsa::Database for Database {
    fn salsa_event(&self, _event: salsa::Event) {}
}

#[salsa::db]
impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn accumulate_once() {
    let mut db = Database::default();

    // Just call accumulate on a base input to see what happens.
    let input = MyInput::new(&db, 2, 3);
    let logs = push_logs::accumulated::<Logs>(&db, input);
    expect![[r#"
        [
            "log_b(0 of 3)",
            "log_b(1 of 3)",
            "log_b(2 of 3)",
            "log_a(0 of 2)",
            "log_a(1 of 2)",
        ]"#]]
    .assert_eq(&format!("{:#?}", logs));
    db.assert_logs(expect![[r#"
        [
            "push_logs(a = 2, b = 3)",
            "push_a_logs(2)",
            "push_b_logs(3)",
        ]"#]])
}

#[test]
fn change_a_and_reaccumulate() {
    let mut db = Database::default();

    // Accumulate logs for `a = 2` and `b = 3`
    let input = MyInput::new(&db, 2, 3);
    let logs = push_logs::accumulated::<Logs>(&db, input);
    expect![[r#"
        [
            "log_b(0 of 3)",
            "log_b(1 of 3)",
            "log_b(2 of 3)",
            "log_a(0 of 2)",
            "log_a(1 of 2)",
        ]"#]]
    .assert_eq(&format!("{:#?}", logs));
    db.assert_logs(expect![[r#"
        [
            "push_logs(a = 2, b = 3)",
            "push_a_logs(2)",
            "push_b_logs(3)",
        ]"#]]);

    // Change to `a = 1`, which means `push_logs` does not call `push_a_logs` at all
    input.set_field_a(&mut db).to(1);
    let logs = push_logs::accumulated::<Logs>(&db, input);
    expect![[r#"
        [
            "log_b(0 of 3)",
            "log_b(1 of 3)",
            "log_b(2 of 3)",
        ]"#]]
    .assert_eq(&format!("{:#?}", logs));
    db.assert_logs(expect![[r#"
        [
            "push_logs(a = 1, b = 3)",
        ]"#]]);
}

#[test]
fn get_a_logs_after_changing_b() {
    let mut db = Database::default();

    // Invoke `push_a_logs` with `a = 2` and `b = 3` (but `b` doesn't matter)
    let input = MyInput::new(&db, 2, 3);
    let logs = push_a_logs::accumulated::<Logs>(&db, input);
    expect![[r#"
        [
            "log_a(0 of 2)",
            "log_a(1 of 2)",
        ]"#]]
    .assert_eq(&format!("{:#?}", logs));
    db.assert_logs(expect![[r#"
        [
            "push_a_logs(2)",
        ]"#]]);

    // Changing `b` does not cause `push_a_logs` to re-execute
    // and we still get the same result
    input.set_field_b(&mut db).to(5);
    let logs = push_a_logs::accumulated::<Logs>(&db, input);
    expect![[r#"
        [
            "log_a(0 of 2)",
            "log_a(1 of 2)",
        ]
    "#]]
    .assert_debug_eq(&logs);
    db.assert_logs(expect!["[]"]);
}
