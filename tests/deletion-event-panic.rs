#![cfg(feature = "inventory")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use salsa::{Database, Setter, Storage};

#[salsa::db]
struct TestDatabase {
    storage: Storage<Self>,
}

impl Default for TestDatabase {
    fn default() -> Self {
        let discards = Arc::new(AtomicUsize::new(0));
        Self {
            storage: Storage::new(Some(Box::new(move |event| {
                if matches!(event.kind, salsa::EventKind::DidDiscard { .. })
                    && discards.fetch_add(1, Ordering::Relaxed) == 1
                {
                    panic!("discard callback panic");
                }
            }))),
        }
    }
}

#[salsa::db]
impl Database for TestDatabase {}

#[salsa::input]
struct Input {
    keep: bool,
}

#[salsa::tracked]
struct Entity<'db> {
    value: u32,
}

#[salsa::tracked]
fn create(db: &dyn Database, input: Input) -> Option<Entity<'_>> {
    input.keep(db).then(|| Entity::new(db, 1))
}

#[salsa::tracked]
fn read_entity(db: &dyn Database, entity: Entity<'_>) -> u32 {
    entity.value(db)
}

#[test]
fn deletion_can_be_retried_after_event_callback_panics() {
    let mut db = TestDatabase::default();
    let input = Input::new(&db, true);
    let entity = create(&db, input).unwrap();
    assert_eq!(read_entity(&db, entity), 1);

    input.set_keep(&mut db).to(false);

    let first = catch_unwind(AssertUnwindSafe(|| create(&db, input)));
    assert!(first.is_err());

    let retry = catch_unwind(AssertUnwindSafe(|| create(&db, input)));
    assert!(retry.unwrap().is_none());
}
