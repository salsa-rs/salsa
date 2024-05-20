//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.

use salsa::DebugWithDb;
use test_log::test;

#[salsa::jar(db = Db)]
struct Jar(MyInput, MyTracked<'_>, create_tracked_list);

trait Db: salsa::DbWithJar<Jar> {}

#[salsa::db(Jar)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

impl Db for Database {}

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
fn create_tracked_list<'db>(db: &'db dyn Db, input: MyInput) -> MyTracked<'db> {
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
                MyTracked(
                    0x00007fc15c011010,
                    PhantomData<&salsa_2022::tracked_struct::ValueStruct<tracked_with_struct_db::__MyTrackedConfig>>,
                ),
            ),
        }
    "#]].assert_debug_eq(&t0.debug(&db));
    assert_eq!(t0, t1);
}
