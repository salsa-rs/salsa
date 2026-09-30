#![cfg(feature = "inventory")]

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn outer(db: &dyn salsa::Database) -> u32 {
    participant(db)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _| 0)]
fn participant(db: &dyn salsa::Database) -> u32 {
    if outer(db) == 0 { 1 } else { participant(db) }
}

#[test_log::test]
fn promoted_participant_reuses_provisional_value() {
    let db = salsa::DatabaseImpl::default();

    // First, `participant` returns 1 with only `outer` as its cycle head.
    // In the next iteration it calls itself and becomes a head. It must reuse
    // its previous 1: inserting a new initial 0 makes `outer` alternate forever,
    // repeatedly demoting and promoting `participant`.
    assert_eq!(outer(&db), 1);
    assert_eq!(participant(&db), 1);
}
