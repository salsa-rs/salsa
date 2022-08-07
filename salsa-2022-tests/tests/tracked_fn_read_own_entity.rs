//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use expect_test::expect;
use salsa_2022_tests::{HasLogger, Logger};
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked, final_result, intermediate_result);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
fn final_result(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result({:?})", input));
    intermediate_result(db, input).field(db) * 2
}

#[salsa::tracked(jar = Jar)]
struct MyTracked {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
fn intermediate_result(db: &dyn Db, input: MyInput) -> MyTracked {
    db.push_log(format!("intermediate_result({:?})", input));
    let tracked = MyTracked::new(db, input.field(db) / 2);
    let _ = tracked.field(db); // read the field of an entity we created
    tracked
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
}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn one_entity() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 22);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "intermediate_result(MyInput(Id { value: 1 }))",
        ]"#]]);

    // Intermediate result is the same, so final result does
    // not need to be recomputed:
    input.set_field(&mut db, 23);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput(Id { value: 1 }))",
        ]"#]]);

    input.set_field(&mut db, 24);
    assert_eq!(final_result(&db, input), 24);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput(Id { value: 1 }))",
            "final_result(MyInput(Id { value: 1 }))",
        ]"#]]);
}

/// Create and mutate a distinct input. No re-execution required.
#[test]
fn red_herring() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 22);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "intermediate_result(MyInput(Id { value: 1 }))",
        ]"#]]);

    // Create a distinct input and mutate it.
    // This will trigger a new revision in the database
    // but shouldn't actually invalidate our existing ones.
    let input2 = MyInput::new(&mut db, 44);
    input2.set_field(&mut db, 66);

    // Re-run the query on the original input. Nothing re-executes!
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
        ]"#]]);
}
