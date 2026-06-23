#![cfg(feature = "inventory")]

use salsa::plumbing::{AsId, ZalsaDatabase};

#[salsa::input(page_size = 128)]
struct Input128 {
    value: usize,
}

#[salsa::input(page_size = 256)]
struct Input256 {
    value: usize,
}

#[salsa::input(page_size = 512)]
struct Input512 {
    value: usize,
}

#[salsa::input(page_size = 256)]
struct EmptyInput256 {
    value: usize,
}

#[cfg(feature = "persistence")]
#[salsa::input(page_size = 128, persist)]
struct PersistedInput128 {
    value: usize,
}

#[cfg(all(feature = "persistence", not(feature = "shuttle")))]
#[test]
fn persistence_preserves_page_class() {
    let mut db = salsa::DatabaseImpl::new();
    let expected = PersistedInput128::new(&db, 128);
    let json = serde_json::to_string(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

    let mut restored = salsa::DatabaseImpl::new();
    <dyn salsa::Database>::deserialize(
        &mut restored,
        &mut serde_json::Deserializer::from_str(&json),
    )
    .unwrap();
    let actual = PersistedInput128::ingredient(&restored)
        .entries(restored.zalsa())
        .last()
        .unwrap()
        .as_struct();
    assert_eq!(actual.as_id(), expected.as_id());
    assert_eq!(*actual.value(&restored), 128);
}

#[salsa::input]
struct Input1024 {
    value: usize,
}

#[salsa::interned(page_size = 128)]
struct Interned128<'db> {
    value: usize,
}

#[salsa::tracked(page_size = 512)]
struct Tracked512<'db> {
    value: usize,
}

#[salsa::tracked]
fn create_tracked(db: &dyn salsa::Database, input: Input128) -> Tracked512<'_> {
    let base = input.value(db);
    (0..513)
        .map(|value| Tracked512::new(db, base + value))
        .last()
        .unwrap()
}

#[test]
fn configurable_page_sizes() {
    #[cfg(feature = "shuttle")]
    shuttle::check_random(configurable_page_sizes_impl, 1);
    #[cfg(not(feature = "shuttle"))]
    configurable_page_sizes_impl();
}

fn configurable_page_sizes_impl() {
    let db = salsa::DatabaseImpl::new();
    let _ = EmptyInput256::ingredient(&db);
    let input128 = (0..129).map(|v| Input128::new(&db, v)).last().unwrap();
    let input256 = (0..257).map(|v| Input256::new(&db, v)).last().unwrap();
    let input512 = (0..513).map(|v| Input512::new(&db, v)).last().unwrap();
    let input1024 = (0..1025).map(|v| Input1024::new(&db, v)).last().unwrap();
    let interned = (0..129).map(|v| Interned128::new(&db, v)).last().unwrap();
    let tracked = create_tracked(&db, input128);

    for (id, class, name) in [
        (input1024.as_id(), 0, "Input1024"),
        (input512.as_id(), 1, "Input512"),
        (tracked.as_id(), 1, "Tracked512"),
        (input256.as_id(), 2, "Input256"),
        (input128.as_id(), 3, "Input128"),
        (interned.as_id(), 3, "Interned128"),
    ] {
        assert_eq!(id.index() >> 30, class);
        assert_eq!(
            db.zalsa()
                .lookup_ingredient(db.zalsa().ingredient_index(id))
                .debug_name(),
            name
        );
    }

    assert_eq!(*input1024.value(&db), 1024);
    assert_eq!(*input512.value(&db), 512);
    assert_eq!(*tracked.value(&db), 640);
    assert_eq!(*input256.value(&db), 256);
    assert_eq!(*input128.value(&db), 128);
    assert_eq!(*interned.value(&db), 128);

    let memory_usage = <dyn salsa::Database>::memory_usage(&db);
    for (name, capacity) in [
        ("Input1024", 1024),
        ("Input512", 512),
        ("Tracked512", 512),
        ("Input256", 256),
        ("EmptyInput256", 256),
        ("Input128", 128),
        ("Interned128", 128),
    ] {
        let ingredient = memory_usage
            .structs
            .iter()
            .find(|ingredient| ingredient.debug_name() == name)
            .unwrap();
        assert_eq!(ingredient.page_info().unwrap().page_capacity(), capacity);
    }
}
