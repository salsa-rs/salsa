//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use std::cell::Cell;

use common::LogDatabase;
use expect_test::expect;
mod common;
use salsa::{Database, Setter};
use test_log::test;

thread_local! {
    static COUNTER: Cell<usize> = const { Cell::new(0) };
}

#[salsa::input]
struct MyInput {
    field1: u32,
    field2: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    counter: usize,
}

#[salsa::tracked]
fn function(db: &dyn Database, input: MyInput) -> usize {
    // Read input 1
    let _field1 = input.field1(db);

    // **BAD:** Leak in the value of the counter non-deterministically
    let counter = COUNTER.with(|c| c.get());

    // Create the tracked struct, which (from salsa's POV), only depends on field1;
    // but which actually depends on the leaked value.
    let tracked = MyTracked::new(db, counter);

    // Read input 2. This will cause us to re-execute on revision 2.
    let _field2 = input.field2(db);

    tracked.counter(db)
}

#[test]
fn test_leaked_inputs_ignored() {
    let mut db = common::EventLoggerDatabase::default();

    let input = MyInput::new(&db, 10, 20);
    let result_in_rev_1 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(0) } }",
        ]"#]]);

    assert_eq!(result_in_rev_1, 0);

    // Modify field2 so that `function` is seen to have changed --
    // but only *after* the tracked struct is created.
    input.set_field2(&mut db).to(30);

    // Also modify the thread-local counter
    COUNTER.with(|c| c.set(100));

    let result_in_rev_2 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { thread_id: ThreadId(2), kind: DidSetCancellationFlag }",
            "Event { thread_id: ThreadId(2), kind: WillCheckCancellation }",
            "Event { thread_id: ThreadId(2), kind: WillExecute { database_key: function(0) } }",
        ]"#]]);

    // Because salsa did not see any way for the tracked
    // struct to have changed, its field values will not have
    // been updated, even though in theory they would have
    // the leaked value from the counter.
    assert_eq!(result_in_rev_2, 0);
}
