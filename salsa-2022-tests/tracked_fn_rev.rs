//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
#![allow(dead_code)]

use expect_test::expect;
use std::cell::RefCell;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked, final_result, intermediate_result);

trait Db: salsa::DbWithJar<Jar> {
    fn push_log(&self, message: String);
}

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
    MyTracked::new(db, input.field(db) / 2)
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    log: RefCell<Vec<String>>,
}

impl Database {
    fn assert_logs(&mut self, expected: expect_test::Expect) {
        let logs = std::mem::replace(&mut *self.log.borrow_mut(), vec![]);
        expected.assert_eq(&format!("{:#?}", logs));
    }
}

impl salsa::Database for Database {
    fn salsa_runtime(&self) -> &salsa::Runtime {
        self.storage.runtime()
    }
}

impl Db for Database {
    fn push_log(&self, message: String) {
        self.log.borrow_mut().push(message);
    }
}

#[test]
fn execute() {
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
