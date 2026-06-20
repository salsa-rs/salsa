#![cfg(all(feature = "inventory", feature = "persistence"))]

use salsa::Setter;

const DELETED_VALUE: &str = "deleted tracked entry sentinel";

#[salsa::input(persist)]
struct Input {
    enabled: bool,
}

#[salsa::tracked(persist)]
struct Entity<'db> {
    value: String,
}

#[salsa::tracked(persist)]
fn maybe_entity(db: &dyn salsa::Database, input: Input) -> Option<Entity<'_>> {
    input
        .enabled(db)
        .then(|| Entity::new(db, DELETED_VALUE.to_owned()))
}

#[salsa::tracked(persist)]
fn consume(db: &dyn salsa::Database, entity: Entity<'_>) -> usize {
    entity.value(db).len()
}

#[test]
fn deleted_tracked_structs_are_not_persisted() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, true);
    assert!(maybe_entity(&db, input).is_some());

    input.set_enabled(&mut db).to(false);
    assert!(maybe_entity(&db, input).is_none());

    let serialized = serde_json::to_string(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();
    assert!(!serialized.contains(DELETED_VALUE));
}

mod stale {
    use super::*;

    #[salsa::input(persist)]
    struct Input {
        value: u32,
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct CollidingValue(u32);

    impl std::hash::Hash for CollidingValue {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            std::hash::Hash::hash(&0_u8, state);
        }
    }

    #[salsa::tracked(persist)]
    struct Entity<'db> {
        value: CollidingValue,
    }

    #[salsa::tracked(persist)]
    fn make_entity(db: &dyn salsa::Database, input: Input) -> Entity<'_> {
        Entity::new(db, CollidingValue(input.value(db)))
    }

    #[salsa::tracked(persist)]
    fn consume(db: &dyn salsa::Database, entity: Entity<'_>) -> u32 {
        entity.value(db).0
    }

    #[test]
    fn serializing_a_stale_value_does_not_mark_it_current() {
        let mut db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, 1);
        let entity = make_entity(&db, input);
        assert_eq!(consume(&db, entity), 1);

        input.set_value(&mut db).to(2);
        serde_json::to_string(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

        assert_eq!(make_entity(&db, input).value(&db).0, 2);
    }
}
