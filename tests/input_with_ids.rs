#![allow(warnings)]

use expect_test::expect;

#[salsa::db]
trait Db: salsa::Database {}

#[derive(Clone, Debug)]
struct Field {}

#[salsa::input]
struct MyInput {
    #[id]
    id_one: u32,
    #[id]
    id_two: u16,

    field: Field,
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl Db for Database {}

#[test]
fn test_debug() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 50, 10, Field {});

    let actual = format!("{:?}", input.debug(&db));
    let expected = expect!["MyInput { [salsa id]: 0, id_one: 50, id_two: 10, field: Field }"];
    expected.assert_eq(&actual);
}

#[test]
fn test_set() {
    let mut db = Database::default();
    let input = MyInput::new(&mut db, 50, 10, Field {});
    input.set_field(&mut db).to(Field {});
}
