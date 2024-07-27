//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

use expect_test::expect;
use test_log::test;

#[salsa::interned]
struct InternedString<'db> {
    data: String,
}

#[salsa::interned]
struct InternedPair<'db> {
    data: (InternedString<'db>, InternedString<'db>),
}

#[salsa::tracked]
fn intern_stuff(db: &dyn salsa::Database) -> String {
    let s1 = InternedString::new(db, "Hello, ".to_string());
    let s2 = InternedString::new(db, "World, ".to_string());
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
