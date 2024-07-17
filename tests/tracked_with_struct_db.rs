//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.

use test_log::test;

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::input]
struct MyInput {
    field: String,
}

#[salsa::tracked]
struct MyTracked<'db> {
    data: MyInput,
    next: MyList<'db>,
}

#[derive(PartialEq, Eq, Clone, Debug, salsa::Update)]
enum MyList<'db> {
    None,
    Next(MyTracked<'db>),
}

#[salsa::tracked]
fn create_tracked_list<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyTracked<'db> {
    let t0 = MyTracked::new(db, input, MyList::None);
    let t1 = MyTracked::new(db, input, MyList::Next(t0));
    t1
}

#[test]
fn execute() {
    let mut db = Database::default();
    let input = MyInput::new(&mut db, "foo".to_string());
    let t0: MyTracked = create_tracked_list(&db, input);
    let t1 = create_tracked_list(&db, input);
    expect_test::expect![[r#"
        MyTracked {
            [salsa id]: 1,
            data: MyInput {
                [salsa id]: 0,
                field: "foo",
            },
            next: Next(
                MyTracked {
                    [salsa id]: 0,
                    data: MyInput {
                        [salsa id]: 0,
                        field: "foo",
                    },
                    next: None,
                },
            ),
        }
    "#]]
    .assert_debug_eq(&t0);
    assert_eq!(t0, t1);
}
