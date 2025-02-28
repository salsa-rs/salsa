//! Test tracked struct output from a query in a cycle.

#[salsa::tracked]
struct Output<'db> {
    value: u32,
}

#[salsa::tracked]
fn read_value<'db>(db: &'db dyn salsa::Database, output: Output<'db>) -> u32 {
    output.value(db)
}

#[salsa::tracked]
fn query_a(db: &dyn salsa::Database) -> u32 {
    let val = query_b(db);
    let output = Output::new(db, val);
    let read = read_value(db, output);
    assert_eq!(read, val);
    if val > 4 {
        val
    } else {
        val + 1
    }
}

#[salsa::tracked(cycle_fn=cycle_fn, cycle_initial=cycle_initial)]
fn query_b(db: &dyn salsa::Database) -> u32 {
    query_a(db)
}

fn cycle_initial(_db: &dyn salsa::Database) -> u32 {
    0
}

fn cycle_fn(
    _db: &dyn salsa::Database,
    _value: &u32,
    _count: u32,
) -> salsa::CycleRecoveryAction<u32> {
    salsa::CycleRecoveryAction::Iterate
}

#[test_log::test]
fn the_test() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(query_b(&db), 5);
}
