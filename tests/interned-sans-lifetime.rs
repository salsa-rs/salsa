use expect_test::expect;
use std::path::{Path, PathBuf};
use test_log::test;

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CustomSalsaIdWrapper(salsa::Id);

impl salsa::plumbing::AsId for CustomSalsaIdWrapper {
    fn as_id(&self) -> salsa::Id {
        self.0
    }
}

impl salsa::plumbing::FromId for CustomSalsaIdWrapper {
    fn from_id(id: salsa::Id) -> Self {
        CustomSalsaIdWrapper(id)
    }
}

#[salsa::interned_sans_lifetime(id = CustomSalsaIdWrapper)]
struct InternedString {
    data: String,
}

#[salsa::interned_sans_lifetime(id = CustomSalsaIdWrapper)]
struct InternedPair {
    data: (InternedString, InternedString),
}

#[salsa::interned_sans_lifetime(id = CustomSalsaIdWrapper)]
struct InternedTwoFields {
    data1: String,
    data2: String,
}

#[salsa::interned_sans_lifetime(id = CustomSalsaIdWrapper)]
struct InternedVec {
    data1: Vec<String>,
}

#[salsa::interned_sans_lifetime]
struct InternedPathBuf {
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

#[salsa::tracked]
fn length(db: &dyn salsa::Database, s: InternedString) -> usize {
    s.data(db).len()
}

#[test]
fn tracked_static_query_works() {
    let db = salsa::DatabaseImpl::new();
    let s1 = InternedString::new(&db, "Hello, World!".to_string());
    assert_eq!(length(&db, s1), 13);
}
