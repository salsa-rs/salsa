//! Test that `DisplayWithDb` works correctly

use std::fmt::Display;

use expect_test::expect;
use salsa::DisplayWithDb;

#[salsa::jar(db = Db)]
struct Jar(MyInput, ComplexStruct);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::input]
struct MyInput {
    field: u32,
}

impl<'db> DisplayWithDb<'db, dyn Db + 'db> for MyInput {
    fn fmt_with(&self, db: &dyn Db, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MyInput({})", self.field(db))
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
struct NotSalsa {
    field: String,
}

impl Display for NotSalsa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NotSalsa({})", self.field)
    }
}

#[salsa::input]
struct ComplexStruct {
    my_input: MyInput,
    not_salsa: NotSalsa,
}

impl<'db> DisplayWithDb<'db, dyn Db + 'db> for ComplexStruct {
    fn fmt_with(&self, db: &dyn Db, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ComplexStruct {{ {}, {} }}",
            self.my_input(db).display_with(db),
            self.not_salsa(db)
        )
    }
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

    // all fields
    let actual = format!("{}", complex_struct.display_with(&db));
    let expected = expect!["ComplexStruct { MyInput(22), NotSalsa(it's salsa time) }"];
    expected.assert_eq(&actual);
}
