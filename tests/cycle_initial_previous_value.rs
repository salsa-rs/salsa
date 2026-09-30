#![cfg(feature = "inventory")]

// A participant becomes a head when a provisional result exposes a new dependency.
// Resetting that participant to zero would make this graph oscillate indefinitely.
// Regression test for https://github.com/salsa-rs/salsa/issues/1349.
#[salsa::tracked(returns(copy), cycle_initial = |_, _, previous: Option<&u8>| previous.copied().unwrap_or(0))]
fn outer(db: &dyn salsa::Database) -> u8 {
    conditional(db).min(1).max(successor(db))
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, previous: Option<&u8>| previous.copied().unwrap_or(0))]
fn successor(db: &dyn salsa::Database) -> u8 {
    (conditional(db) + 1).min(2)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, previous: Option<&u8>| previous.copied().unwrap_or(0))]
fn conditional(db: &dyn salsa::Database) -> u8 {
    let value = outer(db);
    if value >= 2 {
        value.max(successor(db))
    } else {
        value
    }
}

#[test]
fn every_entry_query_converges() {
    for entry in [outer, successor, conditional] {
        let db = salsa::DatabaseImpl::default();
        assert_eq!(entry(&db), 2);
        assert_eq!(outer(&db), 2);
        assert_eq!(successor(&db), 2);
        assert_eq!(conditional(&db), 2);
    }
}
