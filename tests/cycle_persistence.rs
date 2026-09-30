#![cfg(all(feature = "persistence", feature = "inventory"))]

use salsa::{Database, Setter};

#[salsa::input(persist)]
struct Input {
    #[returns(copy)]
    limit: u32,
}

#[salsa::tracked(returns(copy), persist, cycle_initial = |_, _, _| 0)]
fn outer(db: &dyn Database, input: Input) -> u32 {
    inner(db, input)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn inner(db: &dyn Database, input: Input) -> u32 {
    (outer(db, input).max(inner(db, input)) + 1).min(input.limit(db))
}

#[test]
fn finalized_cycle_heads_round_trip() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 3);
    assert_eq!(outer(&db, input), 3);
    assert_eq!(inner(&db, input), 3);

    let serialized = serde_json::to_string(&<dyn Database>::as_serialize(&mut db)).unwrap();
    let mut restored = salsa::DatabaseImpl::default();
    <dyn Database>::deserialize(
        &mut restored,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    assert_eq!(outer(&restored, input), 3);
    assert_eq!(inner(&restored, input), 3);
    input.set_limit(&mut restored).to(4);
    assert_eq!(outer(&restored, input), 4);
    assert_eq!(inner(&restored, input), 4);
}
