#![cfg(all(feature = "persistence", feature = "inventory"))]

mod common;

#[salsa::interned(persist)]
struct SelfRefInterned<'db> {
    field: String,
    #[self_ref]
    other: SelfRefInterned<'db>,
}

#[test]
fn self_ref_interned_round_trip() {
    use salsa::plumbing::AsId;

    let (serialized, root_id, child_id) = {
        let mut db = common::EventLoggerDatabase::default();
        let root = SelfRefInterned::new(&db, "root".to_string(), None);
        let child = SelfRefInterned::new(&db, "child".to_string(), Some(root));
        let root_id = root.as_id();
        let child_id = child.as_id();
        let serialized =
            serde_json::to_string(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

        (serialized, root_id, child_id)
    };

    let mut db = common::EventLoggerDatabase::default();
    <dyn salsa::Database>::deserialize(
        &mut db,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    let root = SelfRefInterned::new(&db, "root", None);
    let child = SelfRefInterned::new(&db, "child", Some(root));

    assert_eq!(root.as_id(), root_id);
    assert_eq!(child.as_id(), child_id);
    assert!(*root.other(&db) == root);
    assert!(*child.other(&db) == root);
}
