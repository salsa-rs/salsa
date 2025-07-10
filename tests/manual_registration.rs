#![cfg(not(feature = "inventory"))]

mod ingredients {
    #[salsa::input]
    pub(super) struct MyInput {
        field: u32,
    }

    #[salsa::tracked]
    pub(super) struct MyTracked<'db> {
        pub(super) field: u32,
    }

    #[salsa::interned]
    pub(super) struct MyInterned<'db> {
        pub(super) field: u32,
    }

    #[salsa::tracked]
    pub(super) fn track<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyInterned<'db> {
        MyInterned::new(db, input.field(db))
    }

    #[salsa::tracked]
    pub(super) fn intern<'db>(db: &'db dyn salsa::Database, input: MyInput) -> MyTracked<'db> {
        MyTracked::new(db, input.field(db))
    }
}

#[test]
fn test() {
    let db = salsa::DatabaseImpl {
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
