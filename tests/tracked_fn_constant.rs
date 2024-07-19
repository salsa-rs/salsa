//! Test that a constant `tracked` fn (has no inputs)
//! compiles and executes successfully.
#![allow(warnings)]

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database) -> u32 {
    44
}

#[test]
fn execute() {
    #[salsa::db]
    #[derive(Default)]
    struct Database {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for Database {}

    let mut db = Database::default();
    assert_eq!(tracked_fn(&db), 44);
}
