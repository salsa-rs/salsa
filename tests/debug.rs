//! Test that `DeriveWithDb` is correctly derived.

use expect_test::expect;
use salsa::{Database, Setter};

#[salsa::input]
struct MyInput {
    field: u32,
}

#[derive(Debug, Eq, PartialEq, Clone)]
struct NotSalsa {
    field: String,
}

#[salsa::input]
struct ComplexStruct {
    my_input: MyInput,
    not_salsa: NotSalsa,
}

#[test]
fn input() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = MyInput::new(db, 22);
        let not_salsa = NotSalsa {
            field: "it's salsa time".to_string(),
        };
        let complex_struct = ComplexStruct::new(db, input, not_salsa);

        // debug includes all fields
        let actual = format!("{complex_struct:?}");
        let expected = expect![[r#"ComplexStruct { [salsa id]: Id(0), my_input: MyInput { [salsa id]: Id(0), field: 22 }, not_salsa: NotSalsa { field: "it's salsa time" } }"#]];
        expected.assert_eq(&actual);
    })
}

#[salsa::tracked]
fn leak_debug_string(_db: &dyn salsa::Database, input: MyInput) -> String {
    format!("{input:?}")
}

/// Test that field reads that occur as part of `Debug` are not tracked.
/// Intentionally leaks the debug string.
/// Don't try this at home, kids.
#[test]
fn untracked_dependencies() {
    let mut db = salsa::DatabaseImpl::new();

    let input = MyInput::new(&db, 22);

    let s = leak_debug_string(&db, input);
    expect![[r#"
        "MyInput { [salsa id]: Id(0), field: 22 }"
    "#]]
    .assert_debug_eq(&s);

    input.set_field(&mut db).to(23);

    // check that we reuse the cached result for debug string
    // even though the dependency changed.
    let s = leak_debug_string(&db, input);
    assert!(s.contains(", field: 22 }"));
}

#[salsa::tracked(no_debug)]
struct DerivedCustom<'db> {
    my_input: MyInput,
    value: u32,
}

impl std::fmt::Debug for DerivedCustom<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        salsa::with_attached_database(|db| {
            write!(f, "{:?} / {:?}", self.my_input(db), self.value(db))
        })
        .unwrap_or_else(|| f.debug_tuple("DerivedCustom").finish())
    }
}

#[salsa::tracked]
fn leak_derived_custom(db: &dyn salsa::Database, input: MyInput, value: u32) -> String {
    let c = DerivedCustom::new(db, input, value);
    format!("{c:?}")
}

#[test]
fn custom_debug_impl() {
    let db = salsa::DatabaseImpl::new();

    let input = MyInput::new(&db, 22);

    let s = leak_derived_custom(&db, input, 23);
    expect![[r#"
        "MyInput { [salsa id]: Id(0), field: 22 } / 23"
    "#]]
    .assert_debug_eq(&s);
}
