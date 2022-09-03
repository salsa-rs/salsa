//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
#![allow(warnings)]

#[salsa::jar(db = Db)]
struct Jar(
    MyInput,
    MyInput_tracked_fn,
    MyInput_tracked_fn_ref,
    MyInput_TrackedTrait_tracked_trait_fn,
);

trait Db: salsa::DbWithJar<Jar> {}

trait TrackedTrait {
    fn tracked_trait_fn(self, db: &dyn Db) -> u32;
}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
impl MyInput {
    #[salsa::tracked]
    fn tracked_fn(self, db: &dyn Db) -> u32 {
        self.field(db) * 2
    }

    #[salsa::tracked(return_ref)]
    fn tracked_fn_ref(self, db: &dyn Db) -> u32 {
        self.field(db) * 3
    }
}

#[salsa::tracked]
impl TrackedTrait for MyInput {
    #[salsa::tracked]
    fn tracked_trait_fn(self, db: &dyn Db) -> u32 {
        self.field(db) * 4
    }
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
    let object = MyInput::new(&mut db, 22);
    assert_eq!(object.tracked_fn(&db), 44);
    assert_eq!(*object.tracked_fn_ref(&db), 66);
    assert_eq!(object.tracked_trait_fn(&db), 88);
}
