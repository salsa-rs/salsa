//! Test that a `tracked` fn on a `salsa::interned`
//! compiles and executes successfully.

#[salsa::interned]
struct Name<'db> {
    name: String,
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn salsa::Database, name: Name<'db>) -> String {
    name.name(db).clone()
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[test]
fn execute() {
    let db = Database::default();
    let name = Name::new(&db, "Salsa".to_string());

    assert_eq!(tracked_fn(&db, name), "Salsa");
}
