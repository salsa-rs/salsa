use expect_test::expect;
mod common;
use common::LogDatabase;
use salsa::Database;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn LogDatabase, input: MyInput) -> u32 {
    db.push_log(format!("tracked_fn({input:?})"));
    let t = MyTracked::new(db, input.field(db) * 2);
    tracked_fn_extra::specify(db, t, 2222);
    tracked_fn_extra(db, t)
}

#[salsa::tracked(specify)]
fn tracked_fn_extra<'db>(db: &dyn LogDatabase, input: MyTracked<'db>) -> u32 {
    db.push_log(format!("tracked_fn_extra({input:?})"));
    0
}

#[test]
fn execute() {
    let mut db = common::LoggerDatabase::default();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input), 2222);
    db.assert_logs(expect![[r#"
        [
            "tracked_fn(MyInput { [salsa id]: Id(0), field: 22 })",
        ]"#]]);

    // A "synthetic write" causes the system to act *as though* some
    // input of durability `durability` has changed.
    db.synthetic_write(salsa::Durability::LOW);

    // Re-run the query on the original input. Nothing re-executes!
    assert_eq!(tracked_fn(&db, input), 2222);
    db.assert_logs(expect!["[]"]);
}
