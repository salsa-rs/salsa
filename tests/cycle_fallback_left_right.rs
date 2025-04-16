/// A test showing the dependence on cycle entry points for `cycle_result` handling.
#[salsa::tracked(cycle_result=cycle_result)]
fn left(db: &dyn salsa::Database) -> u32 {
    10 * right(db)
}

#[salsa::tracked(cycle_result=cycle_result)]
fn right(db: &dyn salsa::Database) -> u32 {
    1 + left(db)
}

fn cycle_result(_db: &dyn salsa::Database) -> u32 {
    0
}

#[test_log::test]
fn left_entry() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(left(&db), 10);
}

#[test_log::test]
fn right_entry() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(right(&db), 1);
}
