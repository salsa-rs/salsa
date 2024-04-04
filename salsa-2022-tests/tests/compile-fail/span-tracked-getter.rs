#[salsa::jar(db = Db)]
pub struct Jar(MyTracked, my_fn);

pub trait Db: salsa::DbWithJar<Jar> {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[salsa::tracked]
pub struct MyTracked {
    field: u32,
}

#[salsa::tracked]
fn my_fn(db: &dyn crate::Db) {
    let x = MyTracked::new(db, 22);
    x.field(22);
}

fn main() {
    let mut db = Database::default();
    my_fn(&db);
}
