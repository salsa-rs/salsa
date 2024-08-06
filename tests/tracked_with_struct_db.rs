//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.

use salsa::{Database, DatabaseImpl};
use test_log::test;

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
fn create_tracked_list(db: &dyn Database, input: MyInput) -> MyTracked<'_> {
    let t0 = MyTracked::new(db, input, MyList::None);
    let t1 = MyTracked::new(db, input, MyList::Next(t0));
    t1
}

#[test]
fn execute() {
    DatabaseImpl::new().attach(|db| {
        let input = MyInput::new(db, "foo".to_string());
        let t0: MyTracked = create_tracked_list(db, input);
        let t1 = create_tracked_list(db, input);
        expect_test::expect![[r#"
            MyTracked {
                [salsa id]: Id(1),
                data: MyInput {
                    [salsa id]: Id(0),
                    field: "foo",
                },
                next: Next(
                    MyTracked {
                        [salsa id]: Id(0),
                        data: MyInput {
                            [salsa id]: Id(0),
                            field: "foo",
                        },
                        next: None,
                    },
                ),
            }
        "#]]
        .assert_debug_eq(&t0);
        assert_eq!(t0, t1);
    })
}
