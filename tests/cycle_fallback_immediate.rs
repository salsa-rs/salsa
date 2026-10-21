#![cfg(feature = "inventory")]

//! It is possible to omit the `cycle_fn`, only specifying `cycle_result` in which case
//! an immediate fallback value is used as the cycle handling opposed to doing a fixpoint resolution.

#[salsa::tracked(returns(copy), cycle_result=db_)]
fn one_o_one(db: &dyn salsa::Database) -> u32 {
    let val = one_o_one(db);
    val + 1
}

// These callback names must not be shadowed by generated adapter parameters.
fn db_(_db: &dyn salsa::Database, _id: salsa::Id) -> u32 {
    100
}

#[test_log::test]
fn simple() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(one_o_one(&db), 100);
}

#[salsa::tracked(returns(copy), cycle_result=id_)]
fn two_queries1(db: &dyn salsa::Database) -> i32 {
    two_queries2(db) + 1
}

#[salsa::tracked(returns(copy), cycle_result=id_)]
fn two_queries2(db: &dyn salsa::Database) -> i32 {
    two_queries1(db)
}

fn id_(_db: &dyn salsa::Database, _id: salsa::Id) -> i32 {
    1
}

#[test]
fn two_queries() {
    let db = salsa::DatabaseImpl::default();

    assert_eq!(two_queries1(&db), 1);
    assert_eq!(two_queries2(&db), 1);
}
