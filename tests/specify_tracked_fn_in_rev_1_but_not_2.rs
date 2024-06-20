//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use expect_test::expect;
mod common;
use common::{HasLogger, Logger};
use salsa::Setter;
use test_log::test;

#[salsa::db]
trait Db: salsa::Database + HasLogger {}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    input: MyInput,
}

/// If the input is in the range 0..10, this is specified to return 10.
/// Otherwise, the default occurs, and it returns the input.
#[salsa::tracked(specify)]
fn maybe_specified<'db>(db: &'db dyn Db, tracked: MyTracked<'db>) -> u32 {
    db.push_log(format!("maybe_specified({:?})", tracked));
    tracked.input(db).field(db)
}

/// Reads maybe-specified and multiplies it by 10.
/// This is here to show whether we can detect when `maybe_specified` has changed
/// and control down-stream work accordingly.
#[salsa::tracked]
fn read_maybe_specified<'db>(db: &'db dyn Db, tracked: MyTracked<'db>) -> u32 {
    db.push_log(format!("read_maybe_specified({:?})", tracked));
    maybe_specified(db, tracked) * 10
}

/// Create a tracked value and *maybe* specify a value for
/// `maybe_specified`
#[salsa::tracked]
fn create_tracked(db: &dyn Db, input: MyInput) -> MyTracked<'_> {
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

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

#[salsa::db]
impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        self.push_log(format!("{event:?}"));
    }
}

#[salsa::db]
impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[test]
fn test_run_0() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 0);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 0 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 0 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 0 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);
}

#[test]
fn test_run_5() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 5 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 5 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 5 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);
}

#[test]
fn test_run_10() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 10);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 10 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 10 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 10 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: maybe_specified(0) } }",
            "maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 10 } })",
        ]"#]]);
}

#[test]
fn test_run_20() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 20);
    assert_eq!(final_result(&db, input), 200);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: maybe_specified(0) } }",
            "maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
        ]"#]]);
}

#[test]
fn test_run_0_then_5_then_20() {
    let mut db = Database::default();

    // Set input to 0:
    //
    // * `create_tracked` specifies `10` for `maybe_specified`
    // * final resuilt of `100` is derived by executing `read_maybe_specified`
    let input = MyInput::new(&db, 0);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 0 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 0 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 0 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);

    // Set input to 5:
    //
    // * `create_tracked` does re-execute, but specifies same value for `maybe_specified` as before
    // * `read_maybe_specified` does not re-execute (its input has not changed)
    input.set_field(&mut db).to(5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 5 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: DidValidateMemoizedValue { database_key: read_maybe_specified(0) } }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: DidValidateMemoizedValue { database_key: final_result(0) } }",
        ]"#]]);

    // Set input to 20:
    //
    // * `create_tracked` re-executes but does not specify any value
    // * `read_maybe_specified` is invoked and it calls `maybe_specified`, which now executes
    //   (its value has not been specified)
    input.set_field(&mut db).to(20);
    assert_eq!(final_result(&db, input), 200);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillDiscardStaleOutput { execute_key: create_tracked(0), output_key: maybe_specified(0) } }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: maybe_specified(0) } }",
            "maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);
}

#[test]
fn test_run_0_then_5_then_10_then_20() {
    let mut db = Database::default();

    // Set input to 0:
    //
    // * `create_tracked` specifies `10` for `maybe_specified`
    // * final resuilt of `100` is derived by executing `read_maybe_specified`
    let input = MyInput::new(&db, 0);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 0 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 0 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 0 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);

    // Set input to 5:
    //
    // * `create_tracked` does re-execute, but specifies same value for `maybe_specified` as before
    // * `read_maybe_specified` does not re-execute (its input has not changed)
    input.set_field(&mut db).to(5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 5 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: DidValidateMemoizedValue { database_key: read_maybe_specified(0) } }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: DidValidateMemoizedValue { database_key: final_result(0) } }",
        ]"#]]);

    // Set input to 10:
    //
    // * `create_tracked` does re-execute and specifies no value for `maybe_specified`
    // * `maybe_specified_value` returns 10; this is the same value as was specified.
    // * `read_maybe_specified` therefore does NOT need to execute.
    input.set_field(&mut db).to(10);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 10 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillDiscardStaleOutput { execute_key: create_tracked(0), output_key: maybe_specified(0) } }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: maybe_specified(0) } }",
            "maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 10 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: DidValidateMemoizedValue { database_key: read_maybe_specified(0) } }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: DidValidateMemoizedValue { database_key: final_result(0) } }",
        ]"#]]);

    // Set input to 20:
    //
    // * Everything re-executes to get new result (200).
    input.set_field(&mut db).to(20);
    assert_eq!(final_result(&db, input), 200);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: maybe_specified(0) } }",
            "maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);
}

#[test]
fn test_run_5_then_20() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 5);
    assert_eq!(final_result(&db, input), 100);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 5 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 5 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 5 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);

    input.set_field(&mut db).to(20);
    assert_eq!(final_result(&db, input), 200);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: create_tracked(0) } }",
            "create_tracked(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillDiscardStaleOutput { execute_key: create_tracked(0), output_key: maybe_specified(0) } }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: maybe_specified(0) } }",
            "maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: read_maybe_specified(0) } }",
            "read_maybe_specified(MyTracked { [salsa id]: Id(0), input: MyInput { [salsa id]: Id(0), field: 20 } })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: final_result(0) } }",
            "final_result(MyInput { [salsa id]: Id(0), field: 20 })",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
        ]"#]]);
}
