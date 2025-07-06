mod common;

use salsa::{Database, Setter};

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
}

#[salsa::tracked]
fn intermediate(db: &dyn salsa::Database, input: MyInput) -> TrackedWithLateField<'_> {
    input.field1(db);
    input.field2(db);
    let t = TrackedWithLateField::new(db, 0, 1, 2, 3);
    t.set_tracked_1(db, input.field1(db));
    t.set_tracked_2(db, input.field2(db));
    t
}

#[salsa::tracked]
fn accumulate(db: &dyn salsa::Database, input: MyInput) -> (usize, usize) {
    let tracked = intermediate(db, input);
    let one = read_tracked_1(db, tracked);
    let two = read_tracked_2(db, tracked);

    (one, two)
}

#[salsa::tracked]
fn read_tracked_1<'db>(db: &'db dyn Database, tracked: TrackedWithLateField<'db>) -> usize {
    tracked.tracked_1(db)
}

#[salsa::tracked]
fn read_tracked_2<'db>(db: &'db dyn Database, tracked: TrackedWithLateField<'db>) -> usize {
    tracked.tracked_2(db)
}

#[test_log::test]
fn execute() {
    let mut db = salsa::DatabaseImpl::default();
    let input = MyInput::new(&db, 1, 1);

    assert_eq!(accumulate(&db, input), (1, 1));

    // Should only re-execute `read_tracked_1`.
    input.set_field1(&mut db).to(2);
    input.set_field2(&mut db).to(1);
    assert_eq!(accumulate(&db, input), (2, 1));

    // Should only re-execute `read_tracked_2`.
    input.set_field2(&mut db).to(2);
    assert_eq!(accumulate(&db, input), (2, 2));
}
