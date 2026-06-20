#![cfg(feature = "inventory")]

use salsa::Setter;

mod deleted {
    use super::*;

    #[salsa::input]
    struct Input {
        enabled: bool,
    }

    #[salsa::tracked]
    struct Entity<'db> {
        value: u32,
    }

    #[salsa::tracked]
    fn maybe_entity(db: &dyn salsa::Database, input: Input) -> Option<Entity<'_>> {
        input.enabled(db).then(|| Entity::new(db, 22))
    }

    #[test]
    fn deleted_tracked_structs_are_not_enumerated() {
        let mut db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, true);
        assert!(maybe_entity(&db, input).is_some());

        input.set_enabled(&mut db).to(false);
        assert!(maybe_entity(&db, input).is_none());

        assert_eq!(Entity::ingredient(&db).entries(&mut db).count(), 0);
    }
}

mod stale {
    use super::*;

    #[salsa::input]
    struct Input {
        value: u32,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CollidingValue(u32);

    impl std::hash::Hash for CollidingValue {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            std::hash::Hash::hash(&0_u8, state);
        }
    }

    #[salsa::tracked]
    struct Entity<'db> {
        value: CollidingValue,
    }

    #[salsa::tracked]
    fn make_entity(db: &dyn salsa::Database, input: Input) -> Entity<'_> {
        Entity::new(db, CollidingValue(input.value(db)))
    }

    #[test]
    fn inspecting_a_stale_value_does_not_mark_it_current() {
        let mut db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, 1);
        assert_eq!(make_entity(&db, input).value(&db).0, 1);

        input.set_value(&mut db).to(2);

        _ = <dyn salsa::Database>::memory_usage(&mut db);

        {
            let entries = Entity::ingredient(&db).entries(&mut db).collect::<Vec<_>>();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].value().fields().0.0, 1);
        }

        assert_eq!(make_entity(&db, input).value(&db).0, 2);
    }
}
