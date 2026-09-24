#![cfg(all(feature = "persistence", feature = "inventory"))]

use salsa::{Database, Setter};

#[salsa::input(persist)]
struct Input {
    #[returns(copy)]
    value: u32,
}

#[salsa::interned(persist)]
struct Value<'db> {
    value: u32,
}

#[test]
fn serialize_stale_interned_query_key() {
    let mut db = salsa::DatabaseImpl::new();
    let input = Input::new(&db, 1);
    let value = create(&db, input);
    assert_eq!(payload(&db, value), 1);

    db.synthetic_write(salsa::Durability::LOW);
    serde_json::to_string(&<dyn Database>::as_serialize(&mut db)).unwrap();
}

#[test]
fn serialize_through_stale_nonpersistent_key() {
    let mut db = salsa::DatabaseImpl::new();
    let input = Input::new(&db, 1);
    assert_eq!(outer(&db, input), 1);
    db.synthetic_write(salsa::Durability::LOW);

    // Flattening the dependency on `inner` reads a memo whose key wasn't enumerated:
    // neither `TransientValue` nor `inner` is persisted.
    let serialized = serde_json::to_string(&<dyn Database>::as_serialize(&mut db)).unwrap();
    let mut restored = salsa::DatabaseImpl::new();
    <dyn Database>::deserialize(
        &mut restored,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    assert_eq!(outer(&restored, input), 1);
    input.set_value(&mut restored).to(2);
    assert_eq!(outer(&restored, input), 2);
}

#[test]
fn restore_into_a_later_revision() {
    let mut source = salsa::DatabaseImpl::new();
    let input = Input::new(&source, 1);
    assert_eq!(payload(&source, create(&source, input)), 1);
    let serialized = serde_json::to_string(&<dyn Database>::as_serialize(&mut source)).unwrap();

    let mut restored = salsa::DatabaseImpl::new();
    restored.synthetic_write(salsa::Durability::LOW);
    // Memos are restored before the saved runtime revision replaces the destination's revision.
    <dyn Database>::deserialize(
        &mut restored,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    assert_eq!(payload(&restored, create(&restored, input)), 1);
}

#[salsa::tracked(returns(copy), persist)]
fn create(db: &dyn Database, input: Input) -> Value<'_> {
    Value::new(db, input.value(db))
}

#[salsa::tracked(returns(copy), persist)]
fn payload(_db: &dyn Database, _value: Value<'_>) -> u32 {
    1
}

#[salsa::interned]
struct TransientValue<'db> {
    #[returns(copy)]
    value: u32,
}

#[salsa::tracked(returns(copy), persist)]
fn outer(db: &dyn Database, input: Input) -> u32 {
    inner(db, TransientValue::new(db, input.value(db)))
}

#[salsa::tracked(returns(copy))]
fn inner(db: &dyn Database, value: TransientValue<'_>) -> u32 {
    value.value(db)
}
