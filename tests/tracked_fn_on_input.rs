//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
#![allow(warnings)]

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: MyInput) -> salsa::Result<u32> {
    Ok(input.field(db)? * 2)
}

#[test]
fn execute() -> salsa::Result<()> {
    let mut db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input)?, 44);

    Ok(())
}
