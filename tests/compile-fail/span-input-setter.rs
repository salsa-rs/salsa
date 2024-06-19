#[salsa::jar(db = Db)]
pub struct Jar(MyInput);

pub trait Db: salsa::DbWithJar<Jar> {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[salsa::input]
pub struct MyInput {
    field: u32,
}

fn main() {
    let mut db = Database::default();
    let input = MyInput::new(&mut db, 22);

    input.field(&db);
    input.set_field(22);
}
