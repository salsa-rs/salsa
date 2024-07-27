//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.
#![allow(warnings)]

trait TrackedTrait {
    fn tracked_trait_fn(self, db: &dyn salsa::Database) -> u32;
}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
impl MyInput {
    #[salsa::tracked]
    fn tracked_fn(self, db: &dyn salsa::Database) -> u32 {
        self.field(db) * 2
    }

    #[salsa::tracked(return_ref)]
    fn tracked_fn_ref(self, db: &dyn salsa::Database) -> u32 {
        self.field(db) * 3
    }
}

#[salsa::tracked]
impl TrackedTrait for MyInput {
    #[salsa::tracked]
    fn tracked_trait_fn(self, db: &dyn salsa::Database) -> u32 {
        self.field(db) * 4
    }
}

#[test]
fn execute() {
    let mut db = salsa::DatabaseImpl::new();
    let object = MyInput::new(&mut db, 22);
    // assert_eq!(object.tracked_fn(&db), 44);
    // assert_eq!(*object.tracked_fn_ref(&db), 66);
    assert_eq!(object.tracked_trait_fn(&db), 88);
}
