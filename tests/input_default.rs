//! Tests that fields attributed with `#[default]` are initialized with `Default::default()`.

use salsa::{default_database, Durability};
use test_log::test;

#[salsa::input]
struct MyInput {
    required: bool,
    #[default]
    optional: usize,
}

#[test]
fn new_constructor() {
    let db = default_database();

    let input = MyInput::new(&db, true);

    assert_eq!(input.required(&db), true);
    assert_eq!(input.optional(&db), 0);
}

#[test]
fn builder_specify_optional() {
    let db = default_database();

    let input = MyInput::builder(true).optional(20).new(&db);

    assert_eq!(input.required(&db), true);
    assert_eq!(input.optional(&db), 20);
}

#[test]
fn builder_default_optional_value() {
    let db = default_database();

    let input = MyInput::builder(true)
        .required_durability(Durability::HIGH)
        .new(&db);

    assert_eq!(input.required(&db), true);
    assert_eq!(input.optional(&db), 0);
}
