//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

#[salsa::jar(db = Db)]
struct Jar(MyInput, tracked_fn);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
struct MyTracked {
    field: u32,
}

#[salsa::tracked(jar = Jar)]
fn tracked_fn(db: &dyn Db, input: MyInput) -> MyTracked {
    MyTracked::new(db, input.field(db) * 2)
}

#[test]
fn execute() {
    #[salsa::db(Jar)]
    #[derive(Default)]
    struct Database {
        storage: salsa::Storage<Self>,
    }

    impl salsa::Database for Database {
        fn salsa_runtime(&self) -> &salsa::Runtime {
            self.storage.runtime()
        }
    }

    impl Db for Database {}

    let mut db = Database::default();
    let input = MyInput::new(&mut db, 22);
    assert_eq!(tracked_fn(&db, input).field(&db), 44);
}
