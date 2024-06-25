#[salsa::jar(db = Db)]
pub struct Jar(a::MyInput);

mod a {
    #[salsa::input(jar = crate::Jar)]
    pub struct MyInput {
        field: u32,
    }
}

pub trait Db: salsa::DbWithJar<Jar> {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

fn main() {
    let mut db = Database::default();
    let input = a::MyInput::new(&mut db, 22);

    input.field(&db);
    input.set_field(&mut db).to(23);
}
