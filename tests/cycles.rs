#[derive(Default)]
pub struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
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

salsa::query_group! {
    trait Database: salsa::Database {
        // `a` and `b` depend on each other and form a cycle
        fn memoized_a() -> () {
            type MemoizedA;
        }
        fn memoized_b() -> () {
            type MemoizedB;
        }
        fn volatile_a() -> () {
            type VolatileA;
            storage volatile;
        }
        fn volatile_b() -> () {
            type VolatileB;
            storage volatile;
        }
    }
}

fn memoized_a(db: &impl Database) -> () {
    db.memoized_b()
}

fn memoized_b(db: &impl Database) -> () {
    db.memoized_a()
}

fn volatile_a(db: &impl Database) -> () {
    db.volatile_b()
}

fn volatile_b(db: &impl Database) -> () {
    db.volatile_a()
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_memoized() {
    let query = DatabaseImpl::default();
    query.memoized_a();
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_volatile() {
    let query = DatabaseImpl::default();
    query.volatile_a();
}
