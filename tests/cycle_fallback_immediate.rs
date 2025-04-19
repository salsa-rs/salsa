//! It is possible to omit the `cycle_fn`, only specifying `cycle_result` in which case
//! an immediate fallback value is used as the cycle handling opposed to doing a fixpoint resolution.
#[salsa::tracked]
fn zero(_db: &dyn salsa::Database) -> u32 {
    0
}

#[salsa::tracked(cycle_result=cycle_result)]
fn one_o_one(db: &dyn salsa::Database) -> u32 {
    let val = one_o_one(db);
    val + 1
}

fn cycle_result(_db: &dyn salsa::Database) -> u32 {
    100
}

#[test_log::test]
fn the_test() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(one_o_one(&db), 101);
}
