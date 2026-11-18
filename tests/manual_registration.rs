#![cfg(not(feature = "inventory"))]

mod ingredients {
    #[salsa::input]
    pub(super) struct MyInput {
        #[returns(copy)]
        field: u32,
    }

    #[salsa::tracked]
    pub(super) struct MyTracked<'db> {
        #[returns(copy)]
        pub(super) field: u32,
    }

    #[salsa::interned]
    pub(super) struct MyInterned<'db> {
        #[returns(copy)]
        pub(super) field: u32,
    }

    #[salsa::tracked(returns(copy))]
    pub(super) fn intern<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyInterned<'db> {
        MyInterned::new(db, input.field(db))
    }

    #[salsa::tracked(returns(copy))]
    pub(super) fn track<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyTracked<'db> {
        MyTracked::new(db, input.field(db))
    }
}

#[salsa::db]
#[derive(Clone, Default)]
pub struct DatabaseImpl {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for DatabaseImpl {}

#[cfg(feature = "persistence")]
mod cache {
    use super::DatabaseImpl;
    use salsa::Setter;

    #[salsa::input(persist)]
    struct Source {
        #[returns(copy)]
        value: u32,
    }

    #[salsa::db]
    trait InnerDb: salsa::Database {}
    #[salsa::db]
    impl InnerDb for DatabaseImpl {}
    #[salsa::db]
    trait OuterDb: InnerDb {}
    #[salsa::db]
    impl OuterDb for DatabaseImpl {}

    #[salsa::tracked(persist, returns(copy))]
    fn inner(db: &dyn InnerDb, source: Source) -> u32 {
        source.value(db) * 2
    }

    #[salsa::tracked(persist, returns(copy))]
    fn outer(db: &dyn OuterDb, source: Source) -> u32 {
        inner(db, source) + 1
    }

    #[test]
    fn cache_requires_the_same_registered_ingredients() {
        let builder = || {
            salsa::Storage::<DatabaseImpl>::builder()
                .ingredient::<Source>()
                .ingredient::<inner>()
                .ingredient::<outer>()
        };
        let mut db = DatabaseImpl {
            storage: builder().build(),
        };
        let source = Source::new(&db, 3);
        assert_eq!(outer(&db, source), 7);
        let root = outer::cache_key(&db, source);
        let mut bytes = Vec::new();
        <dyn salsa::Database>::write_cache(&mut db, &mut bytes, 7, &[root]).unwrap();

        // The query jars require Source even for construction. Reject the cache before
        // installing them, including when a different ingredient takes Source's place.
        let missing_source = || {
            salsa::Storage::<DatabaseImpl>::builder()
                .ingredient::<outer>()
                .ingredient::<inner>()
        };
        for (incompatible, name) in [
            (missing_source(), "Source"),
            (
                missing_source().ingredient::<super::ingredients::MyInput>(),
                "Source",
            ),
            (
                builder().ingredient::<super::ingredients::MyInput>(),
                "MyInput",
            ),
        ] {
            let error = incompatible
                .build_from_cache(bytes.clone(), 7)
                .err()
                .unwrap();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            let message = error.to_string();
            assert!(message.contains(name), "{message}");
        }

        // Reordering and duplicate registration do not change the database layout.
        let db = DatabaseImpl {
            storage: missing_source()
                .ingredient::<Source>()
                .ingredient::<inner>()
                .build_from_cache(bytes, 7)
                .unwrap(),
        };
        assert_eq!(outer(&db, source), 7);
    }

    #[test]
    fn revalidate_manually_registered_database_views() {
        let builder = || {
            salsa::Storage::<DatabaseImpl>::builder()
                .ingredient::<Source>()
                .ingredient::<inner>()
                .ingredient::<outer>()
        };
        let mut db = DatabaseImpl {
            storage: builder().build(),
        };
        let source = Source::new(&db, 3);
        assert_eq!(outer(&db, source), 7);
        let key = outer::cache_key(&db, source);
        let mut bytes = Vec::new();
        <dyn salsa::Database>::write_cache(&mut db, &mut bytes, 7, &[key]).unwrap();
        let mut db = DatabaseImpl {
            storage: builder().build_from_cache(bytes, 7).unwrap(),
        };
        source.set_value(&mut db).to(4);
        assert_eq!(outer(&db, source), 9);
    }
}

#[test]
fn single_database() {
    let db = DatabaseImpl {
        storage: salsa::Storage::builder()
            .ingredient::<ingredients::track>()
            .ingredient::<ingredients::intern>()
            .ingredient::<ingredients::MyInput>()
            .ingredient::<ingredients::MyTracked<'_>>()
            .ingredient::<ingredients::MyInterned<'_>>()
            .build(),
    };

    let input = ingredients::MyInput::new(&db, 1);

    let tracked = ingredients::track(&db, input);
    let interned = ingredients::intern(&db, input);

    assert_eq!(tracked.field(&db), 1);
    assert_eq!(interned.field(&db), 1);
}

#[test]
fn multiple_databases() {
    let db1 = DatabaseImpl {
        storage: salsa::Storage::builder()
            .ingredient::<ingredients::intern>()
            .ingredient::<ingredients::MyInput>()
            .ingredient::<ingredients::MyInterned<'_>>()
            .build(),
    };

    let input = ingredients::MyInput::new(&db1, 1);
    let interned = ingredients::intern(&db1, input);
    assert_eq!(interned.field(&db1), 1);

    // Create a second database with different ingredient indices.
    let db2 = DatabaseImpl {
        storage: salsa::Storage::builder()
            .ingredient::<ingredients::track>()
            .ingredient::<ingredients::intern>()
            .ingredient::<ingredients::MyInput>()
            .ingredient::<ingredients::MyTracked<'_>>()
            .ingredient::<ingredients::MyInterned<'_>>()
            .build(),
    };

    let input = ingredients::MyInput::new(&db2, 2);
    let interned = ingredients::intern(&db2, input);
    assert_eq!(interned.field(&db2), 2);

    let input = ingredients::MyInput::new(&db2, 3);
    let tracked = ingredients::track(&db2, input);
    assert_eq!(tracked.field(&db2), 3);
}
