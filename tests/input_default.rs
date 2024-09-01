//! Tests that fields attributed with `#[default]` are initialized with `Default::default()`.

use salsa::Durability;
use test_log::test;

#[salsa::input]
struct MyInput {
    required: bool,
    #[default]
    optional: usize,
}

#[test]
fn new_constructor() -> salsa::Result<()> {
    let db = salsa::DatabaseImpl::new();

    let input = MyInput::new(&db, true);

    assert!(input.required(&db)?);
    assert_eq!(input.optional(&db)?, 0);
    Ok(())
}

#[test]
fn builder_specify_optional() -> salsa::Result<()> {
    let db = salsa::DatabaseImpl::new();

    let input = MyInput::builder(true).optional(20).new(&db);

    assert!(input.required(&db)?);
    assert_eq!(input.optional(&db)?, 20);

    Ok(())
}

#[test]
fn builder_default_optional_value() -> salsa::Result<()> {
    let db = salsa::DatabaseImpl::new();

    let input = MyInput::builder(true)
        .required_durability(Durability::HIGH)
        .new(&db);

    assert!(input.required(&db)?);
    assert_eq!(input.optional(&db)?, 0);

    Ok(())
}
