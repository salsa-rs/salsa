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
fn test_durability() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, _input: Input) -> Interned<'db> {
        Interned::new(db, 0)
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result_in_rev_1 = function(&db, input);
    assert_eq!(result_in_rev_1.field1(&db), 0);

    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    assert_eq!(result_in_rev_2.field1(&db), 0);
}

#[test]
fn test_durability2() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        let _ = input.field1(db);
        function2(db)
    }

    fn function2<'db>(db: &'db dyn Database) -> Interned<'db> {
        Interned::new(db, 0)
    }

    let mut db = common::EventLoggerDatabase::default();

    let input = Input::new(&db, 0);
    let result_in_rev_1 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(3), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(3), kind: WillExecute { database_key: function(Id(0)) } }",
        ]"#]]);

    assert_eq!(result_in_rev_1.field1(&db), 0);

    // Modify the input to force the value to be re-interned.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(3), kind: DidSetCancellationFlag }",
            "Event { thread_id: ThreadId(3), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(3), kind: WillExecute { database_key: function(Id(0)) } }",
        ]"#]]);

    assert_eq!(result_in_rev_2.field1(&db), 0);
}
