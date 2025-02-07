#[salsa::interned]
struct InternedStruct<'db> {
    name: String,
}

#[salsa::input]
struct InputStruct {
    field: u32,
}

#[salsa::tracked]
struct TrackedStruct<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: InputStruct) -> TrackedStruct<'_> {
    TrackedStruct::new(db, input.field(db) * 2)
}

#[test]
fn execute() {
    let db = salsa::DatabaseImpl::new();

    let _ = InternedStruct::new(&db, "Salsa".to_string());
    let _ = InternedStruct::new(&db, "Salsa2".to_string());

    // test interned structs
    let interned = db
        .storage()
        .debug_interned_entries::<InternedStruct>()
        .collect::<Vec<_>>();

    assert_eq!(interned.len(), 2);
    assert_eq!(interned[0].fields().0, "Salsa");
    assert_eq!(interned[1].fields().0, "Salsa2");

    // test input structs
    let input = InputStruct::new(&db, 22);
    let inputs = db
        .storage()
        .debug_input_entries::<InputStruct>()
        .collect::<Vec<_>>();

    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].fields().0, 22);

    // test tracked structs
    let computed = tracked_fn(&db, input).field(&db);
    assert_eq!(computed, 44);
    let tracked = db
        .storage()
        .debug_tracked_entries::<TrackedStruct>()
        .collect::<Vec<_>>();

    assert_eq!(tracked.len(), 1);
    assert_eq!(tracked[0].fields().0, computed);
}
