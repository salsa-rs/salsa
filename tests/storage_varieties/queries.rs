crate trait Counter: salsa::Database {
    fn increment(&self) -> usize;
}

salsa::query_prototype! {
    crate trait Database: Counter {
        fn memoized() for Memoized;
        fn volatile() for Volatile;
    }
}

salsa::query_definition! {
    /// Because this query is memoized, we only increment the counter
    /// the first time it is invoked.
    crate Memoized(db: &impl Database, (): ()) -> usize {
        db.increment()
    }
}

salsa::query_definition! {
    /// Because this query is volatile, each time it is invoked,
    /// we will increment the counter.
    #[storage(volatile)]
    crate Volatile(db: &impl Database, (): ()) -> usize {
        db.increment()
    }
}
