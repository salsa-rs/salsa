//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use std::cell::Cell;

use expect_test::expect;
use salsa::DebugWithDb;
use salsa_2022_tests::{HasLogger, Logger};
use test_log::test;

thread_local! {
    static COUNTER: Cell<usize> = Cell::new(0);
}

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked, function);

trait Db: salsa::DbWithJar<Jar> + HasLogger {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
    logger: Logger,
}

impl salsa::Database for Database {
    fn salsa_event(&self, event: salsa::Event) {
        self.push_log(format!("{:?}", event.debug(self)));
    }
}

impl Db for Database {}

impl HasLogger for Database {
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

#[salsa::input]
struct MyInput {
    field1: u32,
    field2: u32,
}

#[salsa::tracked]
struct MyTracked {
    counter: usize,
}

#[salsa::tracked]
fn function(db: &dyn Db, input: MyInput) -> usize {
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
    let mut db = Database::default();

    let input = MyInput::new(&db, 10, 20);
    let result_in_rev_1 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: function(0) } }",
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
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillCheckCancellation }",
            "Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: function(0) } }",
        ]"#]]);

    // Because salsa did not see any way for the tracked
    // struct to have changed, its field values will not have
    // been updated, even though in theory they would have
    // the leaked value from the counter.
    assert_eq!(result_in_rev_2, 0);
}
