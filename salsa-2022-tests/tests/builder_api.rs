#![allow(warnings)]

use expect_test::expect;
use salsa::Durability;
#[salsa::jar(db = Db)]
struct Jar(MyInput, MySingletonInput);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input(jar = Jar)]
struct MyInput {
    field1: u32,
    field2: u32,
}

#[salsa::input(singleton)]
struct MySingletonInput {
    field1: u32,
}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
fn test_builder() {
    let mut db = Database::default();
    // durability set with new is always low
    let mut input = MyInput::new(&db, 12, 13);
    input
        .set_field1(&mut db)
        .with_durability(Durability::HIGH)
        .to(20);
    input
        .set_field2(&mut db)
        .with_durability(Durability::HIGH)
        .to(40);
    let input_from_builder = MyInput::new_builder()
        .with_durability(Durability::HIGH)
        .with_fields(20, 40)
        .build(&db)
        .unwrap();

    assert_eq!(input.field1(&db), input_from_builder.field1(&db));
    assert_eq!(input.field2(&db), input_from_builder.field2(&db));
}

#[test]
#[should_panic]
// should panic because were creating he same input twice
fn test_sg_builder_panic() {
    let mut db = Database::default();
    let input1 = MySingletonInput::new(&db, 5);
    let input_from_builder = MySingletonInput::new_builder()
        .with_durability(Durability::LOW)
        .with_fields(5)
        .build(&db)
        .unwrap();
}
