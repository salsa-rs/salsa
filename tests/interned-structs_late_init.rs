#![cfg(feature = "inventory")]

//! Test late-initialized interned fields.

use std::convert::identity;

use salsa::plumbing::AsId;
use test_log::test;

#[salsa::interned]
struct InternedString<'db> {
    data: String,
    #[late_init]
    other: InternedString<'db>,
}

#[salsa::interned]
struct Mixed<'db> {
    #[late_init]
    allocated_id: salsa::Id,
    first_key: String,
    #[late_init]
    sibling: Option<Mixed<'db>>,
    second_key: u32,
}

#[test]
fn late_initialized_field_can_reference_an_interned_value() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedString::new(&db, "Hello, ", identity);
    let s2 = InternedString::new(&db, "World, ", |_| s1);

    assert!(*s1.other(&db) == s1);
    assert!(*s2.other(&db) == s1);
}

#[test]
fn late_initializer_is_not_invoked_for_an_existing_key() {
    let db = salsa::DatabaseImpl::new();
    let value = InternedString::new(&db, "key", identity);
    let same_value = InternedString::new(&db, "key", |_| {
        panic!("late initializer invoked for an existing key")
    });

    assert!(value == same_value);
}

#[test]
fn multiple_interleaved_late_initialized_fields() {
    let db = salsa::DatabaseImpl::new();
    let value = Mixed::new(&db, |this| this.as_id(), "first", |this| Some(this), 1);

    assert!(*value.allocated_id(&db) == value.as_id());
    assert!(value.first_key(&db) == "first");
    assert!(*value.sibling(&db) == Some(value));
    assert!(*value.second_key(&db) == 1);

    let same_value = Mixed::new(
        &db,
        |_| panic!("first late initializer invoked for an existing key"),
        "first",
        |_| panic!("second late initializer invoked for an existing key"),
        1,
    );
    let other_value = Mixed::new(&db, |this| this.as_id(), "first", |this| Some(this), 2);

    assert!(value == same_value);
    assert!(value != other_value);
}
