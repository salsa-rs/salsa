//! Test that a constant `tracked` fn (has no inputs)
//! compiles and executes successfully.
#![allow(warnings)]

#[salsa::jar(db = Db)]
struct Jar(tracked_fn);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::tracked]
fn tracked_fn(db: &dyn Db) -> u32 {
    44
}

#[test]
fn execute() {
    #[salsa::db(Jar)]
    #[derive(Default)]
    struct Database {
        storage: salsa::Storage<Self>,
    }

    impl salsa::Database for Database {}

    impl Db for Database {}

    let mut db = Database::default();
    assert_eq!(tracked_fn(&db), 44);
}
