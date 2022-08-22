//! Test that if field X of an input changes but not field Y,
//! functions that depend on X re-execute, but those depending only on Y do not
//! compiles and executes successfully.
#![allow(dead_code)]

use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;

#[salsa::jar(db = Db)]
struct Jar(MyInput, result_depends_on_x, result_depends_on_y);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input(jar = Jar)]
struct MyInput {
    x: u32,
    y: u32,
}

#[salsa::tracked(jar = Jar)]
fn result_depends_on_x(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("result_depends_on_x({:?})", input));
    input.x(db) + 1
}

#[salsa::tracked(jar = Jar)]
fn result_depends_on_y(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("result_depends_on_y({:?})", input));
    input.y(db) - 1
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime {
        self.storage.runtime_mut()
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
    // result_depends_on_x = x + 1
    // result_depends_on_y = y - 1
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 22, 33);
    assert_eq!(result_depends_on_x(&db, input), 23);
    db.assert_logs(expect![[r#"
        [
            "result_depends_on_x(MyInput(Id { value: 1 }))",
        ]"#]]);

    assert_eq!(result_depends_on_y(&db, input), 32);
    db.assert_logs(expect![[r#"
        [
            "result_depends_on_y(MyInput(Id { value: 1 }))",
        ]"#]]);

    input.set_x(&mut db).to(23);
    // input x changes, so result depends on x needs to be recomputed;
    assert_eq!(result_depends_on_x(&db, input), 24);
    db.assert_logs(expect![[r#"
        [
            "result_depends_on_x(MyInput(Id { value: 1 }))",
        ]"#]]);

    // input y is the same, so result depends on y
    // does not need to be recomputed;
    assert_eq!(result_depends_on_y(&db, input), 32);
    db.assert_logs(expect!["[]"]);
}
