//! Test that a constant `tracked` fn (has no inputs)
//! compiles and executes successfully.
#![allow(warnings)]

use crate::common::LogDatabase;

mod common;

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database) -> salsa::Result<u32> {
    Ok(44)
}

#[salsa::tracked]
fn tracked_custom_db(db: &dyn LogDatabase) -> salsa::Result<u32> {
    Ok(44)
}

#[test]
fn execute() -> salsa::Result<()> {
    let mut db = salsa::DatabaseImpl::new();
    assert_eq!(tracked_fn(&db)?, 44);
    Ok(())
}

#[test]
fn execute_custom() -> salsa::Result<()> {
    let mut db = common::LoggerDatabase::default();
    assert_eq!(tracked_custom_db(&db)?, 44);
    Ok(())
}
