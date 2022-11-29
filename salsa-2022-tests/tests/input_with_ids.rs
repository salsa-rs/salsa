#![allow(warnings)]

use expect_test::expect;
use salsa::DebugWithDb;

#[salsa::jar(db = Db)]
struct Jar(MyInput);

trait Db: salsa::DbWithJar<Jar> {}

#[derive(Clone, Debug)]
struct Field {}

#[salsa::input(jar = Jar)]
struct MyInput {
    #[id]
    id_one: u32,
    #[id]
    id_two: u16,

    field: Field,
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
fn test_debug() {
    let mut db = Database::default();

    let input = MyInput::new(&mut db, 50, 10, Field {});

    let actual = format!("{:?}", input.debug(&db));
    let expected = expect![[r#"MyInput { [salsa id]: 0, id_one: 50, id_two: 10 }"#]];
    expected.assert_eq(&actual);
}

#[test]
fn test_set() {
    let mut db = Database::default();
    let input = MyInput::new(&mut db, 50, 10, Field {});
    input.set_field(&mut db).to(Field {});
}
