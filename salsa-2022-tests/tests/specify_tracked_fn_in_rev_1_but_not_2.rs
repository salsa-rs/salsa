//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use expect_test::expect;
use salsa_2022_tests::{HasLogger, Logger};
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(
    MyInput,
    MyTracked,
    maybe_specified,
    read_maybe_specified,
    create_tracked,
    final_result,
);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked {
    input: MyInput,
}

/// If the input is in the range 0..10, this is specified to return 10.
/// Otherwise, the default occurs, and it returns the input.
#[salsa::tracked(specify)]
fn maybe_specified(db: &dyn Db, tracked: MyTracked) -> u32 {
    db.push_log(format!("maybe_specified({:?})", tracked));
    tracked.input(db).field(db)
}

/// Reads maybe-specified and multiplies it by 10.
/// This is here to show whether we can detect when `maybe_specified` has changed
/// and control down-stream work accordingly.
#[salsa::tracked]
fn read_maybe_specified(db: &dyn Db, tracked: MyTracked) -> u32 {
    db.push_log(format!("read_maybe_specified({:?})", tracked));
    maybe_specified(db, tracked) * 10
}

/// Create a tracked value and *maybe* specify a value for
/// `maybe_specified`
#[salsa::tracked(jar = Jar)]
fn create_tracked(db: &dyn Db, input: MyInput) -> MyTracked {
    db.push_log(format!("create_tracked({:?})", input));
    let tracked = MyTracked::new(db, input);
    if input.field(db) < 10 {
        maybe_specified::specify(db, tracked, 10);
    }
    tracked
}

#[salsa::tracked]
fn final_result(db: &dyn Db, input: MyInput) -> u32 {
    db.push_log(format!("final_result({:?})", input));
    let tracked = create_tracked(db, input);
    read_maybe_specified(db, tracked)
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
fn test_run_0() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 0);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);
}

#[test]
fn test_run_5() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);
}

#[test]
fn test_run_10() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 10);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
            "maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);
}

#[test]
fn test_run_20() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 20);
    assert_eq!(final_result(&db, input), 200);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
            "maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);
}

#[test]
fn test_run_0_then_5_then_20() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 0);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);

    // FIXME: read_maybe_specified should not re-execute
    let input = MyInput::new(&mut db, 5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 2 }))",
            "create_tracked(MyInput(Id { value: 2 }))",
            "read_maybe_specified(MyTracked(Id { value: 2 }))",
        ]"#]]);

    input.set_field(&mut db, 20);
    assert_eq!(final_result(&db, input), 100); // FIXME: Should be 20.
    db.assert_logs(expect![[r#"
        [
            "create_tracked(MyInput(Id { value: 2 }))",
        ]"#]]); // FIXME: should invoke maybe_specified
}

#[test]
fn test_run_0_then_5_then_10_then_20() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 0);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);

    // FIXME: `read_maybe_specified` should not re-execute
    let input = MyInput::new(&mut db, 5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 2 }))",
            "create_tracked(MyInput(Id { value: 2 }))",
            "read_maybe_specified(MyTracked(Id { value: 2 }))",
        ]"#]]);

    // FIXME: should execute `maybe_specified` but not `read_maybe_specified`
    let input = MyInput::new(&mut db, 10);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 3 }))",
            "create_tracked(MyInput(Id { value: 3 }))",
            "read_maybe_specified(MyTracked(Id { value: 3 }))",
            "maybe_specified(MyTracked(Id { value: 3 }))",
        ]"#]]);

    // FIXME: should execute `maybe_specified` but not `read_maybe_specified`
    input.set_field(&mut db, 20);
    assert_eq!(final_result(&db, input), 200);
    db.assert_logs(expect![[r#"
        [
            "create_tracked(MyInput(Id { value: 3 }))",
            "maybe_specified(MyTracked(Id { value: 3 }))",
            "read_maybe_specified(MyTracked(Id { value: 3 }))",
            "final_result(MyInput(Id { value: 3 }))",
        ]"#]]);
}

#[test]
fn test_run_5_then_20() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput(Id { value: 1 }))",
            "create_tracked(MyInput(Id { value: 1 }))",
            "read_maybe_specified(MyTracked(Id { value: 1 }))",
        ]"#]]);

    input.set_field(&mut db, 20);
    assert_eq!(final_result(&db, input), 100); // FIXME: Should be 20.
    db.assert_logs(expect![[r#"
        [
            "create_tracked(MyInput(Id { value: 1 }))",
        ]"#]]); // FIXME: should invoke maybe_specified
}
