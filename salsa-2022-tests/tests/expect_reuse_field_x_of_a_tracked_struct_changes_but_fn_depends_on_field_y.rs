//! Test that if field X of a tracked struct changes but not field Y,
//! functions that depend on X re-execute, but those depending only on Y do not
//! compiles and executes successfully.
#![allow(dead_code)]

use salsa_2022_tests::{HasLogger, Logger};

use expect_test::expect;

#[salsa::jar(db = Db)]
struct Jar(
    MyInput,
    MyTracked,
    final_result_depends_on_x,
    final_result_depends_on_y,
    intermediate_result,
);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
fn final_result_depends_on_x(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result_depends_on_x({:?})", input));
    intermediate_result(db, input).x(db) * 2
}

#[salsa::tracked(jar = Jar)]
fn final_result_depends_on_y(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result_depends_on_y({:?})", input));
    intermediate_result(db, input).y(db) * 2
}

#[salsa::tracked(jar = Jar)]
struct MyTracked {
    x: u32,
    y: u32,
}

#[salsa::tracked(jar = Jar)]
fn intermediate_result(db: &dyn Db, input: MyInput) -> MyTracked {
    MyTracked::new(db, (input.field(db) + 1) / 2, input.field(db) / 2)
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
    // x = (input.field + 1) / 2
    // y = input.field / 2
    // final_result_depends_on_x = x * 2 = (input.field + 1) / 2 * 2
    // final_result_depends_on_y = y * 2 = input.field / 2 * 2
    let mut db = Database::default();

    // intermediate results:
    // x = (22 + 1) / 2 = 11
    // y = 22 / 2 = 11
    let input = MyInput::new(&mut db, 22);
    assert_eq!(final_result_depends_on_x(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result_depends_on_x(MyInput(Id { value: 1 }))",
        ]"#]]);

    assert_eq!(final_result_depends_on_y(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result_depends_on_y(MyInput(Id { value: 1 }))",
        ]"#]]);

    input.set_field(&mut db).to(23);
    // x = (23 + 1) / 2 = 12
    // Intermediate result x changes, so final result depends on x
    // needs to be recomputed;
    assert_eq!(final_result_depends_on_x(&db, input), 24);
    db.assert_logs(expect![[r#"
        [
            "final_result_depends_on_x(MyInput(Id { value: 1 }))",
        ]"#]]);

    // y = 23 / 2 = 11
    // Intermediate result y is the same, so final result depends on y
    // does not need to be recomputed;
    assert_eq!(final_result_depends_on_y(&db, input), 22);
    db.assert_logs(expect!["[]"]);
}
