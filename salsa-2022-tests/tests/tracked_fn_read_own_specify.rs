#![allow(warnings)]

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked, tracked_fn, tracked_fn_extra);

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
fn tracked_fn(db: &dyn Db, input: MyInput) -> u32 {
    let t = MyTracked::new(db, input.field(db) * 2);
    tracked_fn_extra::specify(db, t, 2222);
    tracked_fn_extra(db, t)
}

#[salsa::tracked(jar = Jar, specify)]
fn tracked_fn_extra(_db: &dyn Db, _input: MyTracked) -> u32 {
    0
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
fn execute() {
    let mut db = Database::default();
    let input = MyInput::new(&mut db, 22);
    assert_eq!(tracked_fn(&db, input), 2222);

    let input2 = MyInput::new(&mut db, 44);
    assert_eq!(tracked_fn(&db, input), 2222);
}
