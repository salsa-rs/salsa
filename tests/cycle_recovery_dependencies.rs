#![cfg(feature = "inventory")]

//! Queries or inputs read within the cycle recovery function
//! are tracked on the cycle function and don't "leak" into the
//! function calling the query with cycle handling.

use expect_test::expect;
use salsa::Setter as _;

use crate::common::LogDatabase;

mod common;

#[salsa::input]
struct Input {
    value: u32,
}

#[salsa::tracked]
fn entry(db: &dyn salsa::Database, input: Input) -> u32 {
    query(db, input)
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query(db: &dyn salsa::Database, input: Input) -> u32 {
    let val = query(db, input);
    if val < 5 {
        val + 1
    } else {
        val
    }
}

fn cycle_initial(_db: &dyn salsa::Database, _id: salsa::Id, _input: Input) -> u32 {
    0
}

fn cycle_fn(
    db: &dyn salsa::Database,
    _cycle: salsa::Cycle<'_, u32>,
    value: u32,
    input: Input,
) -> u32 {
    let _input = input.value(db);
    value
}

#[test_log::test]
fn the_test() {
    let mut db = common::EventLoggerDatabase::default();

    let input = Input::new(&db, 1);
    assert_eq!(entry(&db, input), 5);

    db.assert_logs_len(15);

    input.set_value(&mut db).to(2);

    assert_eq!(entry(&db, input), 5);
    db.assert_logs(expect![[r#"
        [
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillExecute { database_key: query(Id(0)) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: query(Id(0)), iteration_count: IterationCount(1) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: query(Id(0)), iteration_count: IterationCount(2) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: query(Id(0)), iteration_count: IterationCount(3) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: query(Id(0)), iteration_count: IterationCount(4) }",
            "WillCheckCancellation",
            "WillIterateCycle { database_key: query(Id(0)), iteration_count: IterationCount(5) }",
            "WillCheckCancellation",
            "DidValidateMemoizedValue { database_key: entry(Id(0)) }",
        ]"#]]);
}
