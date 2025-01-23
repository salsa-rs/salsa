//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use expect_test::expect;
use std::path::{Path, PathBuf};
use test_log::test;

#[salsa::interned]
struct InternedBoxed<'db> {
    data: Box<str>,
}

#[salsa::interned]
struct InternedString<'db> {
    data: String,
}

#[salsa::interned]
struct InternedPair<'db> {
    data: (InternedString<'db>, InternedString<'db>),
}

#[salsa::interned]
struct InternedTwoFields<'db> {
    data1: String,
    data2: String,
}

#[salsa::interned]
struct InternedVec<'db> {
    data1: Vec<String>,
}

#[salsa::interned]
struct InternedPathBuf<'db> {
    data1: PathBuf,
}

#[salsa::tracked]
fn intern_stuff(db: &dyn salsa::Database) -> String {
    let s1 = InternedString::new(db, "Hello, ".to_string());
    let s2 = InternedString::new(db, "World, ");
    let s3 = InternedPair::new(db, (s1, s2));

    format!("{s3:?}")
}

#[test]
fn execute() {
    let db = salsa::DatabaseImpl::new();
    expect![[r#"
        "InternedPair { data: (InternedString { data: \"Hello, \" }, InternedString { data: \"World, \" }) }"
    "#]].assert_debug_eq(&intern_stuff(&db));
}

#[test]
fn interning_returns_equal_keys_for_equal_data() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedString::new(&db, "Hello, ".to_string());
    let s2 = InternedString::new(&db, "World, ".to_string());
    let s1_2 = InternedString::new(&db, "Hello, ");
    let s2_2 = InternedString::new(&db, "World, ");
    assert_eq!(s1, s1_2);
    assert_eq!(s2, s2_2);
}
#[test]
fn interning_returns_equal_keys_for_equal_data_multi_field() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedTwoFields::new(&db, "Hello, ".to_string(), "World");
    let s2 = InternedTwoFields::new(&db, "World, ", "Hello".to_string());
    let s1_2 = InternedTwoFields::new(&db, "Hello, ", "World");
    let s2_2 = InternedTwoFields::new(&db, "World, ", "Hello");
    let new = InternedTwoFields::new(&db, "Hello, World", "");

    assert_eq!(s1, s1_2);
    assert_eq!(s2, s2_2);
    assert_ne!(s1, s2_2);
    assert_ne!(s1, new);
}

#[test]
fn interning_boxed() {
    let db = salsa::DatabaseImpl::new();

    assert_eq!(
        InternedBoxed::new(&db, "Hello"),
        InternedBoxed::new(&db, Box::from("Hello"))
    );
}

#[test]
fn interned_structs_have_public_ingredients() {
    use salsa::plumbing::AsId;

    let db = salsa::DatabaseImpl::new();
    let s = InternedString::new(&db, String::from("Hello, world!"));
    let underlying_id = s.0;

    let data = InternedString::ingredient(&db).data(&db, underlying_id.as_id());
    assert_eq!(data, &(String::from("Hello, world!"),));
}

#[test]
fn interning_vec() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedVec::new(&db, ["Hello, ".to_string(), "World".to_string()].as_slice());
    let s2 = InternedVec::new(&db, ["Hello, ", "World"].as_slice());
    let s3 = InternedVec::new(&db, vec!["Hello, ".to_string(), "World".to_string()]);
    let s4 = InternedVec::new(&db, ["Hello, ", "World"].as_slice());
    let s5 = InternedVec::new(&db, ["Hello, ", "World", "Test"].as_slice());
    let s6 = InternedVec::new(&db, ["Hello, ", "World", ""].as_slice());
    let s7 = InternedVec::new(&db, ["Hello, "].as_slice());
    assert_eq!(s1, s2);
    assert_eq!(s1, s3);
    assert_eq!(s1, s4);
    assert_ne!(s1, s5);
    assert_ne!(s1, s6);
    assert_ne!(s5, s6);
    assert_ne!(s6, s7);
}

#[test]
fn interning_path_buf() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedPathBuf::new(&db, PathBuf::from("test_path".to_string()));
    let s2 = InternedPathBuf::new(&db, Path::new("test_path"));
    let s3 = InternedPathBuf::new(&db, Path::new("test_path/"));
    let s4 = InternedPathBuf::new(&db, Path::new("test_path/a"));
    assert_eq!(s1, s2);
    assert_eq!(s1, s3);
    assert_ne!(s1, s4);
}
