#![feature(crate_visibility_modifier)]

#[derive(Default)]
pub struct DatabaseImpl {
    runtime: salsa::runtime::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<DatabaseImpl> {
        &self.runtime
    }
}

salsa::database_storage! {
    pub struct DatabaseImplStorage for DatabaseImpl {
        impl Database {
            fn memoized_a() for MemoizedA;
            fn memoized_b() for MemoizedB;
            fn volatile_a() for VolatileA;
            fn volatile_b() for VolatileB;
        }
    }
}

salsa::query_prototype! {
    trait Database: salsa::Database {
        // `a` and `b` depend on each other and form a cycle
        fn memoized_a(key: ()) -> () {
            type MemoizedA;
        }
        fn memoized_b(key: ()) -> () {
            type MemoizedB;
        }
        fn volatile_a(key: ()) -> () {
            type VolatileA;
        }
        fn volatile_b(key: ()) -> () {
            type VolatileB;
        }
    }
}

salsa::query_definition! {
    crate MemoizedA(db: &impl Database, (): ()) -> () {
        db.memoized_b(())
    }
}

salsa::query_definition! {
    crate MemoizedB(db: &impl Database, (): ()) -> () {
        db.memoized_a(())
    }
}

salsa::query_definition! {
    #[storage(volatile)]
    crate VolatileA(db: &impl Database, (): ()) -> () {
        db.volatile_b(())
    }
}

salsa::query_definition! {
    #[storage(volatile)]
    crate VolatileB(db: &impl Database, (): ()) -> () {
        db.volatile_a(())
    }
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_memoized() {
    let query = DatabaseImpl::default();
    query.memoized_a(());
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_volatile() {
    let query = DatabaseImpl::default();
    query.volatile_a(());
}
