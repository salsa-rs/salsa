//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use common::LogDatabase;
use expect_test::expect;
use salsa::Setter;
use test_log::test;

#[salsa::input(debug)]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn final_result(db: &dyn LogDatabase, input: MyInput) -> u32 {
    db.push_log(format!("final_result({input:?})"));
    intermediate_result(db, input).field(db) * 2
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn intermediate_result(db: &dyn LogDatabase, input: MyInput) -> MyTracked<'_> {
    db.push_log(format!("intermediate_result({input:?})"));
    MyTracked::new(db, input.field(db) / 2)
}

#[test]
fn execute() {
    let mut db = common::LoggerDatabase::default();

    let input = MyInput::new(&db, 22);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput { [salsa id]: Id(0), field: 22 })",
            "intermediate_result(MyInput { [salsa id]: Id(0), field: 22 })",
        ]"#]]);

    // Intermediate result is the same, so final result does
    // not need to be recomputed:
    input.set_field(&mut db).to(23);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput { [salsa id]: Id(0), field: 23 })",
        ]"#]]);

    input.set_field(&mut db).to(24);
    assert_eq!(final_result(&db, input), 24);
    db.assert_logs(expect![[r#"
        [
            "intermediate_result(MyInput { [salsa id]: Id(0), field: 24 })",
            "final_result(MyInput { [salsa id]: Id(0), field: 24 })",
        ]"#]]);
}

/// Create and mutate a distinct input. No re-execution required.
#[test]
fn red_herring() {
    let mut db = common::LoggerDatabase::default();

    let input = MyInput::new(&db, 22);
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        [
            "final_result(MyInput { [salsa id]: Id(0), field: 22 })",
            "intermediate_result(MyInput { [salsa id]: Id(0), field: 22 })",
        ]"#]]);

    // Create a distinct input and mutate it.
    // This will trigger a new revision in the database
    // but shouldn't actually invalidate our existing ones.
    let input2 = MyInput::new(&db, 44);
    input2.set_field(&mut db).to(66);

    // Re-run the query on the original input. Nothing re-executes!
    assert_eq!(final_result(&db, input), 22);
    db.assert_logs(expect![[r#"
        []"#]]);
}
