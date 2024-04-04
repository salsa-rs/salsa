//! Test that `DeriveWithDb` is correctly derived.

use expect_test::expect;
use salsa::DebugWithDb;

#[salsa::jar(db = Db)]
struct Jar(
    MyInput,
    ComplexStruct,
    leak_debug_string,
    DerivedCustom,
    leak_derived_custom,
);

trait Db: salsa::DbWithJar<Jar> {}

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

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

#[test]
fn input() {
    let db = Database::default();

    let input = MyInput::new(&db, 22);
    let not_salsa = NotSalsa {
        field: "it's salsa time".to_string(),
    };
    let complex_struct = ComplexStruct::new(&db, input, not_salsa);

    // debug includes all fields
    let actual = format!("{:?}", complex_struct.debug(&db));
    let expected = expect![[
        r#"ComplexStruct { [salsa id]: 0, my_input: MyInput { [salsa id]: 0, field: 22 }, not_salsa: NotSalsa { field: "it's salsa time" } }"#
    ]];
    expected.assert_eq(&actual);
}

#[salsa::tracked]
fn leak_debug_string(db: &dyn Db, input: MyInput) -> String {
    format!("{:?}", input.debug(db))
}

/// Test that field reads that occur as part of `Debug` are not tracked.
/// Intentionally leaks the debug string.
/// Don't try this at home, kids.
#[test]
fn untracked_dependencies() {
    let mut db = Database::default();

    let input = MyInput::new(&db, 22);

    let s = leak_debug_string(&db, input);
    expect![[r#"
        "MyInput { [salsa id]: 0, field: 22 }"
    "#]]
    .assert_debug_eq(&s);

    input.set_field(&mut db).to(23);

    // check that we reuse the cached result for debug string
    // even though the dependency changed.
    let s = leak_debug_string(&db, input);
    assert!(s.contains(", field: 22 }"));
}

#[salsa::tracked]
#[customize(DebugWithDb)]
struct DerivedCustom {
    my_input: MyInput,
    value: u32,
}

impl DebugWithDb<dyn Db + '_> for DerivedCustom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &dyn Db) -> std::fmt::Result {
        write!(
            f,
            "{:?} / {:?}",
            self.my_input(db).debug(db),
            self.value(db)
        )
    }
}

#[salsa::tracked]
fn leak_derived_custom(db: &dyn Db, input: MyInput, value: u32) -> String {
    let c = DerivedCustom::new(db, input, value);
    format!("{:?}", c.debug(db))
}

#[test]
fn custom_debug_impl() {
    let db = Database::default();

    let input = MyInput::new(&db, 22);

    let s = leak_derived_custom(&db, input, 23);
    expect![[r#"
        "MyInput { [salsa id]: 0, field: 22 } / 23"
    "#]]
    .assert_debug_eq(&s);
}
