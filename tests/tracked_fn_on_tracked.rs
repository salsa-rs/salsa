//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: MyInput) -> salsa::Result<MyTracked<'_>> {
    MyTracked::new(db, input.field(db)? * 2)
}

#[test]
fn execute() -> salsa::Result<()> {
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input)?.field(&db)?, 44);
    Ok(())
}
