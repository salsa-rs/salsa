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
fn incomplete_struct(
    db: &dyn salsa::Database,
    input: MyInput,
    set_twice: bool,
) -> TrackedWithLateField<'_> {
    input.field1(db);
    input.field2(db);
    let t = TrackedWithLateField::new(db, 0, 1, 2, 3);
    t.set_tracked_1(db, input.field1(db));
    if set_twice {
        t.set_tracked_1(db, 123);
    }
    t
}

#[salsa::tracked]
fn read_defined<'db>(db: &'db dyn salsa::Database, t: TrackedWithLateField<'db>) -> usize {
    t.tracked_1(db)
}

#[salsa::tracked]
fn read_undefined<'db>(db: &'db dyn salsa::Database, t: TrackedWithLateField<'db>) -> usize {
    t.tracked_2(db)
}

#[salsa::tracked]
fn setter_query<'db>(
    db: &'db dyn salsa::Database,
    t: TrackedWithLateField<'db>,
) -> TrackedWithLateField<'db> {
    t.set_tracked_1(db, 123);
    t
}

#[test]
#[should_panic]
fn set_late_field_on_foreign_struct() {
    let db = salsa::DatabaseImpl::default();
    let input = MyInput::new(&db, 1, 1);
    let t = incomplete_struct(&db, input, false);
    setter_query(&db, t);
}

#[test]
#[should_panic]
fn set_late_field_twice() {
    let db = salsa::DatabaseImpl::default();
    let input = MyInput::new(&db, 1, 1);
    incomplete_struct(&db, input, true);
}

#[test]
fn read_partially_initialized_struct() {
    let db = salsa::DatabaseImpl::default();
    let input = MyInput::new(&db, 1, 1);
    let t = incomplete_struct(&db, input, false);
    assert_eq!(read_defined(&db, t), 1);
}

#[test]
#[should_panic]
fn read_undefined_late_field() {
    let db = salsa::DatabaseImpl::default();
    let input = MyInput::new(&db, 1, 1);
    let t = incomplete_struct(&db, input, false);
    assert_eq!(read_defined(&db, t), 1);
    read_undefined(&db, t);
}
