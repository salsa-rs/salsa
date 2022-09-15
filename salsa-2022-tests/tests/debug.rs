//! Test that `DeriveWithDb` is correctly derived.

use expect_test::expect;
use salsa::DebugWithDb;

#[salsa::jar(db = Db)]
struct Jar(MyInput, ComplexStruct);

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

    // default debug only includes identity fields
    let actual = format!("{:?}", complex_struct.debug(&db));
    let expected = expect!["ComplexStruct { [salsa id]: 0 }"];
    expected.assert_eq(&actual);

    // all fields
    let actual = format!("{:?}", complex_struct.debug_all(&db));
    let expected = expect![[
        r#"ComplexStruct { [salsa id]: 0, my_input: MyInput { [salsa id]: 0, field: 22 }, not_salsa: NotSalsa { field: "it's salsa time" } }"#
    ]];
    expected.assert_eq(&actual);
}
