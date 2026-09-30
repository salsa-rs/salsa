#![cfg(feature = "inventory")]

mod common;

use expect_test::expect;

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
