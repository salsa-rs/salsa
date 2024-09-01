//! Test that a `tracked` fn on a `salsa::interned`
//! compiles and executes successfully.

#[salsa::interned]
struct Name<'db> {
    name: String,
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn salsa::Database, name: Name<'db>) -> salsa::Result<String> {
    Ok(name.name(db).clone())
}

#[test]
fn execute() -> salsa::Result<()> {
    let db = salsa::DatabaseImpl::new();
    let name = Name::new(&db, "Salsa".to_string())?;

    assert_eq!(tracked_fn(&db, name)?, "Salsa");

    Ok(())
}
