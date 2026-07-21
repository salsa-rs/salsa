#![cfg(feature = "inventory")]

//! Test late-initialized interned fields.

use std::convert::identity;

use test_log::test;

#[salsa::interned]
struct InternedString<'db> {
    data: String,
    #[late_init]
    other: InternedString<'db>,
}

#[test]
fn interning_returns_equal_keys_for_equal_data() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedString::new(&db, "Hello, ".to_string(), identity);
    let s2 = InternedString::new(&db, "World, ".to_string(), |_| s1);
    let s1_2 = InternedString::new(&db, "Hello, ", identity);
    let s2_2 = InternedString::new(&db, "World, ", |_| s2);

    assert!(s1 == s1_2);
    assert!(s2 == s2_2);
    assert!(*s1.other(&db) == s1);
    assert!(*s2.other(&db) == s1);
}
