use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked, tracked_fn);

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
    MyTracked::new(db, input.field(db) / 2)
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

    let input1 = MyInput::new(&db, 22);
    let input2 = MyInput::new(&db, 44);
    let _tracked1 = tracked_fn(&db, input1);
    let _tracked2 = tracked_fn(&db, input2);

    // modify the input and change the revision
    input1.set_field(&mut db).to(24);
    let tracked2 = tracked_fn(&db, input2);

    tracked2.field(&db);
}
