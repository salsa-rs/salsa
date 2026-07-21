#![cfg(all(feature = "persistence", feature = "inventory"))]

use salsa::plumbing::AsId;

#[salsa::interned(persist, revisions = usize::MAX)]
struct Name<'db> {
    text: String,
}

#[test]
fn immortal_persistence_restores_generation_and_metadata() {
    let mut db = salsa::DatabaseImpl::default();
    let name = Name::new(&db, "name".to_owned());
    let id = name.as_id();

    let mut serialized =
        serde_json::to_value(<dyn salsa::Database>::as_serialize(&mut db)).unwrap();
    let ingredient = serialized["ingredients"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .find(|ingredient| {
            ingredient.as_object().is_some_and(|values| {
                values
                    .values()
                    .any(|value| value["fields"] == serde_json::json!(["name"]))
            })
        })
        .unwrap()
        .as_object_mut()
        .unwrap();

    let mut value = ingredient.remove(&id.as_bits().to_string()).unwrap();
    assert_eq!(value["durability"], 3);
    assert_eq!(value["last_interned_at"], u64::MAX);

    // Metadata persisted before eviction was disabled is still accepted.
    value["durability"] = serde_json::json!(0);
    value["last_interned_at"] = serde_json::json!(1);

    let restored_id = id.with_generation(7);
    ingredient.insert(restored_id.as_bits().to_string(), value);

    let serialized = serde_json::to_string(&serialized).unwrap();
    let mut restored = salsa::DatabaseImpl::default();
    <dyn salsa::Database>::deserialize(
        &mut restored,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    let name = Name::new(&restored, "name".to_owned());
    assert_eq!(name.as_id(), restored_id);
    assert_eq!(name.text(&restored), "name");

    let reserialized =
        serde_json::to_value(<dyn salsa::Database>::as_serialize(&mut restored)).unwrap();
    let value = reserialized["ingredients"]
        .as_object()
        .unwrap()
        .values()
        .find_map(|ingredient| ingredient.get(restored_id.as_bits().to_string()))
        .unwrap();
    assert_eq!(value["durability"], 3);
    assert_eq!(value["last_interned_at"], u64::MAX);
}
