//! Tests that code using the builder's durability methods compiles.

use salsa::{default_database, Durability};
use test_log::test;

#[salsa::input]
struct MyInput {
    required_field: bool,

    #[default]
    optional_field: usize,
}

#[test]
fn required_field_durability() {
    let db = default_database();

    let input = MyInput::builder(true)
        .required_field_durability(Durability::HIGH)
        .new(&db);

    assert_eq!(input.required_field(&db), true);
    assert_eq!(input.optional_field(&db), 0);
}

#[test]
fn optional_field_durability() {
    let db = default_database();

    let input = MyInput::builder(true)
        .optional_field(20)
        .optional_field_durability(Durability::HIGH)
        .new(&db);

    assert_eq!(input.required_field(&db), true);
    assert_eq!(input.optional_field(&db), 20);
}
