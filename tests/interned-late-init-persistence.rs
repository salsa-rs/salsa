#![cfg(all(feature = "persistence", feature = "inventory"))]

mod common;

#[salsa::interned(persist)]
struct PersistentLateInitialized<'db> {
    key: String,
    #[returns(copy)]
    #[late_init]
    self_reference: PersistentLateInitialized<'db>,
}

#[test]
fn persistent_late_initialized_self_reference() {
    let mut db = common::LoggerDatabase::default();
    let value = PersistentLateInitialized::new(&db, "key", |this| this);
    assert!(value.self_reference(&db) == value);

    let serialized = serde_json::to_string(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

    let mut db = common::LoggerDatabase::default();
    <dyn salsa::Database>::deserialize(
        &mut db,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    let value = PersistentLateInitialized::new(&db, "key", |_| {
        panic!("late initializer invoked for a restored value")
    });
    assert!(value.self_reference(&db) == value);
}
