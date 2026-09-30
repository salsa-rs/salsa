#![cfg(feature = "inventory")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use expect_test::expect;
use salsa::{Database, Durability, Setter};

use common::{LogDatabase, LoggerDatabase};

// A participant becomes a head when a provisional result exposes a new dependency.
// Resetting that participant to zero would make this graph oscillate indefinitely.
// Regression test for https://github.com/salsa-rs/salsa/issues/1349.
#[salsa::tracked(returns(copy), cycle_initial = |_, _, previous: Option<&u8>| previous.copied().unwrap_or(0))]
fn outer(db: &dyn LogDatabase) -> u8 {
    conditional(db).min(1).max(successor(db))
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, previous: Option<&u8>| previous.copied().unwrap_or(0))]
fn successor(db: &dyn LogDatabase) -> u8 {
    (conditional(db) + 1).min(2)
}

#[salsa::tracked(returns(copy), cycle_initial = conditional_initial)]
fn conditional(db: &dyn LogDatabase) -> u8 {
    let value = outer(db);
    if value >= 2 {
        value.max(successor(db))
    } else {
        value
    }
}

fn conditional_initial(
    db: &dyn LogDatabase,
    _id: salsa::Id,
    last_provisional_value: Option<&u8>,
) -> u8 {
    db.push_log(format!("conditional initial: {last_provisional_value:?}"));
    last_provisional_value.copied().unwrap_or(0)
}

#[salsa::input]
struct Input {
    abort: Arc<AtomicBool>,
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _, _| 0)]
fn aborting_outer(db: &dyn LogDatabase, input: Input) -> u8 {
    let value = participant(db, input);
    if input.abort(db).swap(false, Ordering::Relaxed) {
        panic!("abort after caching a participant value");
    }
    value
}

#[salsa::tracked(returns(copy), cycle_initial = participant_initial)]
fn participant(db: &dyn LogDatabase, input: Input) -> u8 {
    (aborting_outer(db, input) + 1).min(2)
}

fn participant_initial(
    db: &dyn LogDatabase,
    _id: salsa::Id,
    last_provisional_value: Option<&u8>,
    _input: Input,
) -> u8 {
    db.push_log(format!("participant initial: {last_provisional_value:?}"));
    last_provisional_value.copied().unwrap_or(0)
}

#[test]
fn participant_becoming_head_receives_its_previous_value() {
    let db = LoggerDatabase::default();

    assert_eq!(outer(&db), 2);
    assert_eq!(successor(&db), 2);
    assert_eq!(conditional(&db), 2);
    db.assert_logs(expect![[r#"
        [
            "conditional initial: Some(1)",
        ]"#]]);
}

#[test]
fn every_entry_query_converges() {
    for entry in [outer, successor, conditional] {
        let db = LoggerDatabase::default();
        assert_eq!(entry(&db), 2);
        assert_eq!(outer(&db), 2);
        assert_eq!(successor(&db), 2);
        assert_eq!(conditional(&db), 2);
    }
}

#[test]
fn first_execution_and_previous_revision_receive_none() {
    let mut db = LoggerDatabase::default();
    let input = Input::new(&db, Arc::new(AtomicBool::new(false)));

    assert_eq!(participant(&db, input), 2);
    input
        .set_abort(&mut db)
        .to(Arc::new(AtomicBool::new(false)));
    assert_eq!(participant(&db, input), 2);

    db.assert_logs(expect![[r#"
        [
            "participant initial: None",
            "participant initial: None",
        ]"#]]);
}

#[test]
fn previous_revision_does_not_supply_a_provisional_value() {
    let (mut db, input) = aborted_cycle();
    db.synthetic_write(Durability::HIGH);

    assert_eq!(participant(&db, input), 2);
    db.assert_logs(expect![[r#"
        [
            "participant initial: None",
        ]"#]]);
}

#[test]
fn cancelled_computation_does_not_supply_a_provisional_value() {
    let (mut db, input) = aborted_cycle();
    // LRU eviction cancels the old computation without advancing the revision.
    db.trigger_lru_eviction();

    assert_eq!(participant(&db, input), 2);
    db.assert_logs(expect![[r#"
        [
            "participant initial: None",
        ]"#]]);
}

fn aborted_cycle() -> (LoggerDatabase, Input) {
    let db = LoggerDatabase::default();
    let input = Input::new(&db, Arc::new(AtomicBool::new(true)));
    let panic = catch_unwind(AssertUnwindSafe(|| aborting_outer(&db, input))).unwrap_err();
    assert_eq!(
        panic.downcast_ref::<&str>(),
        Some(&"abort after caching a participant value")
    );
    (db, input)
}
