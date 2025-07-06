mod common;

use expect_test::expect;
use salsa::{Database, Setter};

use crate::common::{LogDatabase, LoggerDatabase};

// A tracked struct with mixed tracked and untracked fields to ensure
// the correct field indices are used when tracking dependencies.
#[salsa::tracked(debug)]
struct TrackedWithLateField<'db> {
    untracked_1: usize,

    #[late]
    tracked_1: usize,

    #[late]
    tracked_2: usize,

    untracked_2: usize,

    untracked_3: usize,

    untracked_4: usize,
}

#[salsa::input]
struct MyInput {
    field1: usize,
    field2: usize,
    unused_input: usize,
}

#[salsa::tracked]
fn intermediate(db: &dyn LogDatabase, input: MyInput) -> TrackedWithLateField<'_> {
    db.push_log("intermediate".to_owned());
    let t = TrackedWithLateField::new(db, 0, 1, 2, 3);
    if input.unused_input(db) != 2 {
        t.set_tracked_1(db, input.field1(db));
        t.set_tracked_2(db, input.field2(db));
    }
    input.unused_input(db);
    t
}

#[salsa::tracked]
fn accumulate(db: &dyn LogDatabase, input: MyInput) -> (usize, usize) {
    db.push_log("accumulate".to_owned());
    let tracked = intermediate(db, input);
    let one = read_tracked_1(db, tracked);
    let two = read_tracked_2(db, tracked);

    (one, two)
}

#[salsa::tracked]
fn read_tracked_1<'db>(db: &'db dyn LogDatabase, tracked: TrackedWithLateField<'db>) -> usize {
    db.push_log("read_tracked_1".to_owned());
    tracked.tracked_1(db)
}

#[salsa::tracked]
fn read_tracked_2<'db>(db: &'db dyn LogDatabase, tracked: TrackedWithLateField<'db>) -> usize {
    db.push_log("read_tracked_2".to_owned());
    tracked.tracked_2(db)
}

#[test_log::test]
fn execute() {
    let mut db = LoggerDatabase::default();
    let input = MyInput::new(&db, 1, 1, 0);

    assert_eq!(accumulate(&db, input), (1, 1));

    // Should only re-execute `read_tracked_1`.
    input.set_field1(&mut db).to(2);
    input.set_field2(&mut db).to(1);
    assert_eq!(accumulate(&db, input), (2, 1));

    // Should only re-execute `read_tracked_2`.
    input.set_field2(&mut db).to(2);
    assert_eq!(accumulate(&db, input), (2, 2));
}

#[test]
fn late_field_backdate() {
    let mut db = LoggerDatabase::default();
    let input = MyInput::new(&db, 1, 1, 0);
    accumulate(&db, input);

    db.assert_logs(expect![[r#"
        [
            "accumulate",
            "intermediate",
            "read_tracked_1",
            "read_tracked_2",
        ]"#]]);

    // A "synthetic write" causes the system to act *as though* some
    // input of durability `durability` has changed.
    db.synthetic_write(salsa::Durability::LOW);

    input.set_unused_input(&mut db).to(1);

    // Re-run the query on the original input. Nothing re-executes!
    accumulate(&db, input);
    db.assert_logs(expect![
        r#"
    [
        "intermediate",
    ]"#
    ]);
}
