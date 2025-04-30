//! Test for cycle where only the first iteration of a query depends on the input value.
mod common;

use crate::common::EventLoggerDatabase;
use salsa::{CycleRecoveryAction, Database, Durability, Setter};

#[salsa::input(debug)]
struct Input {
    value: u32,
    max: u32,
}

#[salsa::interned(debug)]
struct Output<'db> {
    #[return_ref]
    value: u32,
}

#[salsa::tracked(cycle_fn=query_a_recover, cycle_initial=query_a_initial)]
fn query_a<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
    query_b(db, input)
}

// Query b also gets low durability because of query_a. How can we avoid that?
// Or is the bug that we loose the durability somehow?
#[salsa::tracked]
fn query_b<'db>(db: &'db dyn salsa::Database, input: Input) -> u32 {
    let value = query_a(db, input);

    if value < input.max(db) {
        // Only the first iteration depends on value but the entire
        // cycle must re-run if input changes.
        let result = value + input.value(db);
        Output::new(db, result);
        result
    } else {
        value
    }
}

// Note: Also requires same output or backdating won't happen.  but other query output needs to be different at least once to fixpint
fn query_a_initial<'db>(db: &'db dyn Database, input: Input) -> u32 {
    0
}

fn query_a_recover<'db>(
    _db: &'db dyn Database,
    _output: &u32,
    _count: u32,
    _input: Input,
) -> CycleRecoveryAction<u32> {
    CycleRecoveryAction::Iterate
}

#[test_log::test]
fn main() {
    let mut db = EventLoggerDatabase::default();

    let input = Input::builder(4, 5).durability(Durability::MEDIUM).new(&db);

    {
        let result = query_a(&db, input);

        assert_eq!(result, 8);
    }

    {
        input.set_value(&mut db).to(3);

        let result = query_a(&db, input);
        assert_eq!(result, 6);
    }
}
