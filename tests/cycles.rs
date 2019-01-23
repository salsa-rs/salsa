#[derive(Default)]
struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }
}

salsa::database_storage! {
    struct DatabaseImplStorage for DatabaseImpl {
        impl Database {
            fn memoized_a() for MemoizedAQuery;
            fn memoized_b() for MemoizedBQuery;
            fn volatile_a() for VolatileAQuery;
            fn volatile_b() for VolatileBQuery;
        }
    }
}

#[salsa::query_group]
trait Database: salsa::Database {
    // `a` and `b` depend on each other and form a cycle
    fn memoized_a(&self) -> ();
    fn memoized_b(&self) -> ();
    #[salsa::volatile]
    fn volatile_a(&self) -> ();
    #[salsa::volatile]
    fn volatile_b(&self) -> ();
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
