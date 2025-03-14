//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use common::LogDatabase;
use expect_test::expect;
use salsa::{Database, Setter};
use test_log::test;

#[salsa::input]
struct Input {
    field1: usize,
}

#[salsa::interned]
struct Interned<'db> {
    field1: usize,
}

#[test]
fn test_intern_new() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        Interned::new(db, input.field1(db))
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result_in_rev_1 = function(&db, input);
    assert_eq!(result_in_rev_1.field1(&db), 0);

    // Modify the input to force a new value to be created.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    assert_eq!(result_in_rev_2.field1(&db), 1);

    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(Id(0)) } }",
            "Event { thread_id: ThreadId(2), kind: DidInternValue { id: Id(400), revision: R1 } }",
            "Event { thread_id: ThreadId(2), kind: DidSetCancellationFlag }",
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(Id(0)) } }",
            "Event { thread_id: ThreadId(2), kind: DidInternValue { id: Id(401), revision: R2 } }",
        ]"#]]);
}

#[test]
fn test_reintern() {
    #[salsa::tracked]
    fn function(db: &dyn Database, input: Input) -> Interned<'_> {
        let _ = input.field1(db);
        Interned::new(db, 0)
    }

    let mut db = common::EventLoggerDatabase::default();

    let input = Input::new(&db, 0);
    let result_in_rev_1 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(Id(0)) } }",
            "Event { thread_id: ThreadId(2), kind: DidInternValue { id: Id(400), revision: R1 } }",
        ]"#]]);

    assert_eq!(result_in_rev_1.field1(&db), 0);

    // Modify the input to force the value to be re-interned.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(2), kind: DidSetCancellationFlag }",
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(Id(0)) } }",
            "Event { thread_id: ThreadId(2), kind: DidReinternValue { id: Id(400), revision: R2 } }",
        ]"#]]);

    assert_eq!(result_in_rev_2.field1(&db), 0);
}

#[test]
fn test_durability() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, _input: Input) -> Interned<'db> {
        Interned::new(db, 0)
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result_in_rev_1 = function(&db, input);
    assert_eq!(result_in_rev_1.field1(&db), 0);

    // Modify the input to bump the revision without re-interning the value, as there
    // is no read dependency.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    assert_eq!(result_in_rev_2.field1(&db), 0);

    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(Id(0)) } }",
            "Event { thread_id: ThreadId(2), kind: DidInternValue { id: Id(400), revision: R1 } }",
            "Event { thread_id: ThreadId(2), kind: DidSetCancellationFlag }",
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: DidValidateMemoizedValue { database_key: function(Id(0)) } }",
        ]"#]]);
}
