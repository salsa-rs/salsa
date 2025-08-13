#![cfg(feature = "inventory")]

//! Test that a `tracked` fn on a `salsa::input`
//! compiles and executes successfully.

mod common;
use common::LogDatabase;
use expect_test::expect;
use salsa::{Database, Durability, Setter};
use test_log::test;

#[salsa::input]
struct Input {
    field1: usize,
}

#[salsa::interned(revisions = 3)]
#[derive(Debug)]
struct Interned<'db> {
    field1: BadHash,
}
// Use a consistent hash value to ensure that interned value sharding
// does not interefere with garbage collection.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
struct BadHash(usize);

impl std::hash::Hash for BadHash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_i16(0);
    }
}

#[salsa::interned]
#[derive(Debug)]
struct NestedInterned<'db> {
    interned: Interned<'db>,
}

#[test]
fn test_intern_new() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        Interned::new(db, BadHash(input.field1(db)))
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result_in_rev_1 = function(&db, input);
    assert_eq!(result_in_rev_1.field1(&db).0, 0);

    // Modify the input to force a new value to be created.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    assert_eq!(result_in_rev_2.field1(&db).0, 1);

    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(400)), revision: R1 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(401)), revision: R2 }",
        ]"#]]);
}

#[test]
fn test_reintern() {
    #[salsa::tracked]
    fn function(db: &dyn Database, input: Input) -> Interned<'_> {
        let _ = input.field1(db);
        Interned::new(db, BadHash(0))
    }

    let mut db = common::EventLoggerDatabase::default();

    let input = Input::new(&db, 0);
    let result_in_rev_1 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(400)), revision: R1 }",
        ]"#]]);

    assert_eq!(result_in_rev_1.field1(&db).0, 0);

    // Modify the input to force the value to be re-interned.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    db.assert_logs(expect![[r#"
        [
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidValidateInternedValue { key: Interned(Id(400)), revision: R2 }",
        ]"#]]);

    assert_eq!(result_in_rev_2.field1(&db).0, 0);
}

#[test]
fn test_durability() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, _input: Input) -> Interned<'db> {
        Interned::new(db, BadHash(0))
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result_in_rev_1 = function(&db, input);
    assert_eq!(result_in_rev_1.field1(&db).0, 0);

    // Modify the input to bump the revision without re-interning the value, as there
    // is no read dependency.
    input.set_field1(&mut db).to(1);

    let result_in_rev_2 = function(&db, input);
    assert_eq!(result_in_rev_2.field1(&db).0, 0);

    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(400)), revision: R1 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "DidValidateMemoizedValue { database_key: function(Id(0)) }",
        ]"#]]);
}

#[salsa::interned(revisions = usize::MAX)]
#[derive(Debug)]
struct Immortal<'db> {
    field1: BadHash,
}

#[test]
fn test_immortal() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, input: Input) -> Immortal<'db> {
        Immortal::new(db, BadHash(input.field1(db)))
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result = function(&db, input);
    assert_eq!(result.field1(&db).0, 0);

    // Modify the input to bump the revision and intern a new value.
    //
    // No values should ever be reused with `durability = usize::MAX`.
    for i in 1..100 {
        input.set_field1(&mut db).to(i);
        let result = function(&db, input);
        assert_eq!(result.field1(&db).0, i);
        assert_eq!(salsa::plumbing::AsId::as_id(&result).generation(), 0);
    }
}

#[test]
fn test_reuse() {
    #[salsa::tracked]
    fn function<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        Interned::new(db, BadHash(input.field1(db)))
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    let result = function(&db, input);
    assert_eq!(result.field1(&db).0, 0);

    // Modify the input to bump the revision and intern a new value.
    //
    // The slot will not be reused for the first few revisions, but after
    // that we should not allocate any more slots.
    for i in 1..10 {
        input.set_field1(&mut db).to(i);

        let result = function(&db, input);
        assert_eq!(result.field1(&db).0, i);
    }

    // Values that have been reused should be re-interned.
    for i in 1..10 {
        let result = function(&db, Input::new(&db, i));
        assert_eq!(result.field1(&db).0, i);
    }

    db.assert_logs(expect![[r#"
        [
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(400)), revision: R1 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(401)), revision: R2 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidInternValue { key: Interned(Id(402)), revision: R3 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(400g1)), revision: R4 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(401g1)), revision: R5 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(402g1)), revision: R6 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(400g2)), revision: R7 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(401g2)), revision: R8 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(402g2)), revision: R9 }",
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(0)) }",
            "DidReuseInternedValue { key: Interned(Id(400g3)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(1)) }",
            "DidInternValue { key: Interned(Id(403)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(2)) }",
            "DidInternValue { key: Interned(Id(404)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(3)) }",
            "DidInternValue { key: Interned(Id(405)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(4)) }",
            "DidInternValue { key: Interned(Id(406)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(5)) }",
            "DidInternValue { key: Interned(Id(407)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(6)) }",
            "DidInternValue { key: Interned(Id(408)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(7)) }",
            "DidValidateInternedValue { key: Interned(Id(401g2)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(8)) }",
            "DidValidateInternedValue { key: Interned(Id(402g2)), revision: R10 }",
            "WillCheckCancellation",
            "WillExecute { database_key: function(Id(9)) }",
        ]"#]]);
}

#[test]
fn test_reuse_indirect() {
    #[salsa::tracked]
    fn intern<'db>(db: &'db dyn Database, input: Input, value: usize) -> Interned<'db> {
        intern_inner(db, input, value)
    }

    #[salsa::tracked]
    fn intern_inner<'db>(db: &'db dyn Database, input: Input, value: usize) -> Interned<'db> {
        let _i = input.field1(db); // Only low durability interned values are garbage collected.
        Interned::new(db, BadHash(value))
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::builder(0).durability(Durability::LOW).new(&db);

    // Intern `i0`.
    let i0 = intern(&db, input, 0);
    let i0_id = salsa::plumbing::AsId::as_id(&i0);
    assert_eq!(i0.field1(&db).0, 0);

    // Get the garbage collector to consider `i0` stale.
    for x in 1.. {
        db.synthetic_write(Durability::LOW);

        let ix = intern(&db, input, x);
        let ix_id = salsa::plumbing::AsId::as_id(&ix);

        // We reused the slot of `i0`.
        if ix_id.index() == i0_id.index() {
            assert_eq!(ix.field1(&db).0, x);

            // Re-intern and read `i0` from a new slot.
            //
            // Note that the only writes have been synthetic, so none of the query dependencies
            // have changed directly. The interned value dependency should be enough to force
            // the inner query to update.
            let i0 = intern(&db, input, 0);
            assert_eq!(i0.field1(&db).0, 0);

            break;
        }
    }
}

#[test]
fn test_reuse_interned_input() {
    // A query that creates an interned value.
    #[salsa::tracked]
    fn create_interned<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        Interned::new(db, BadHash(input.field1(db)))
    }

    #[salsa::tracked]
    fn use_interned<'db>(db: &'db dyn Database, interned: Interned<'db>) -> usize {
        interned.field1(db).0
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    // Create and use I0 in R0.
    let interned = create_interned(&db, input);
    let result = use_interned(&db, interned);
    assert_eq!(result, 0);

    // Create and use I1 in a number of revisions, marking I0 as stale.
    input.set_field1(&mut db).to(1);
    for _ in 0..10 {
        let interned = create_interned(&db, input);
        let result = use_interned(&db, interned);
        assert_eq!(result, 1);

        // Trigger a new revision.
        input.set_field1(&mut db).to(1);
    }

    // Create I2, reusing the stale slot of I0.
    input.set_field1(&mut db).to(2);
    let interned = create_interned(&db, input);

    // Use I2. The function should not be memoized with the value of I0, despite I2 and I0
    // sharing the same slot.
    let result = use_interned(&db, interned);
    assert_eq!(result, 2);
}

#[test]
fn test_reuse_multiple_interned_input() {
    // A query that creates an interned value.
    #[salsa::tracked]
    fn create_interned<'db>(db: &'db dyn Database, input: Input) -> Interned<'db> {
        Interned::new(db, BadHash(input.field1(db)))
    }

    // A query that creates an interned value.
    #[salsa::tracked]
    fn create_nested_interned<'db>(
        db: &'db dyn Database,
        interned: Interned<'db>,
    ) -> NestedInterned<'db> {
        NestedInterned::new(db, interned)
    }

    #[salsa::tracked]
    fn use_interned<'db>(db: &'db dyn Database, interned: Interned<'db>) -> usize {
        interned.field1(db).0
    }

    // A query that reads an interned value.
    #[salsa::tracked]
    fn use_nested_interned<'db>(
        db: &'db dyn Database,
        nested_interned: NestedInterned<'db>,
    ) -> usize {
        nested_interned.interned(db).field1(db).0
    }

    let mut db = common::EventLoggerDatabase::default();
    let input = Input::new(&db, 0);

    // Create and use NI0, which wraps I0, in R0.
    let interned = create_interned(&db, input);
    let i0_id = salsa::plumbing::AsId::as_id(&interned);
    let nested_interned = create_nested_interned(&db, interned);
    let result = use_nested_interned(&db, nested_interned);
    assert_eq!(result, 0);

    // Create and use I1 in a number of revisions, marking I0 as stale.
    input.set_field1(&mut db).to(1);
    for _ in 0..10 {
        let interned = create_interned(&db, input);
        let result = use_interned(&db, interned);
        assert_eq!(result, 1);

        // Trigger a new revision.
        input.set_field1(&mut db).to(1);
    }

    // Create I2, reusing the stale slot of I0.
    input.set_field1(&mut db).to(2);
    let interned = create_interned(&db, input);

    let i2_id = salsa::plumbing::AsId::as_id(&interned);
    assert_ne!(i0_id, i2_id);

    // Create NI1 wrapping I2 instead of I0.
    let nested_interned = create_nested_interned(&db, interned);

    // Use NI1. The function should not be memoized with the value of NI0,
    // despite I2 and I0 sharing the same ID.
    let result = use_nested_interned(&db, nested_interned);
    assert_eq!(result, 2);
}

#[test]
fn test_durability_increase() {
    #[salsa::tracked]
    fn intern<'db>(db: &'db dyn Database, input: Input, value: usize) -> Interned<'db> {
        let _f = input.field1(db);
        Interned::new(db, BadHash(value))
    }

    let mut db = common::EventLoggerDatabase::default();

    let high_durability = Input::builder(0).durability(Durability::HIGH).new(&db);
    let low_durability = Input::builder(1).durability(Durability::LOW).new(&db);

    // Intern `i0`.
    let _i0 = intern(&db, low_durability, 0);
    // Re-intern `i0`, this time using a high-durability.
    let _i0 = intern(&db, high_durability, 0);

    // Get the garbage collector to consider `i0` stale.
    for _ in 0..100 {
        let _dummy = intern(&db, low_durability, 1000).field1(&db);
        db.synthetic_write(Durability::LOW);
    }

    // Intern `i1`.
    //
    // The slot of `i0` should not be reused as it is high-durability, and there
    // were no high-durability writes.
    let _i1 = intern(&db, low_durability, 1);

    // Re-intern and read `i0`.
    //
    // If the slot was reused, the memo would be shallow-verified and we would
    // read `i1` incorrectly.
    let value = intern(&db, high_durability, 0);
    assert_eq!(value.field1(&db).0, 0);

    db.synthetic_write(Durability::LOW);

    // We should have the same issue even after a low-durability write.
    let value = intern(&db, high_durability, 0);
    assert_eq!(value.field1(&db).0, 0);
}
