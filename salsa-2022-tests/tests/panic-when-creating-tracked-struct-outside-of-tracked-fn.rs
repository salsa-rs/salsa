//! Test that creating a tracked struct outside of a
//! tracked function panics with an assert message.

#[salsa::jar(db = Db)]
struct Jar(MyTracked);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::tracked(jar = Jar)]
struct MyTracked {
    field: u32,
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
#[should_panic(
    expected = "cannot create a tracked struct disambiguator outside of a tracked function"
)]
fn execute() {
    let db = Database::default();
    MyTracked::new(&db, 0);
}
