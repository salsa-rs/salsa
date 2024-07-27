//! Test that a constant `tracked` fn (has no inputs)
//! compiles and executes successfully.
#![allow(warnings)]

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database) -> u32 {
    44
}

#[test]
fn execute() {
    let mut db = salsa::DatabaseImpl::new();
    assert_eq!(tracked_fn(&db), 44);
}
