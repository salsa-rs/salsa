#![cfg(all(feature = "persistence", feature = "inventory"))]

use salsa::Database;

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

#[salsa::tracked(returns(copy), persist)]
fn create(db: &dyn Database, input: Input) -> Value<'_> {
    Value::new(db, input.value(db))
}

#[salsa::tracked(returns(copy), persist)]
fn payload(_db: &dyn Database, _value: Value<'_>) -> u32 {
    1
}
