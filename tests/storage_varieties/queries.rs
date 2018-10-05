crate trait Counter: salsa::Database {
    fn increment(&self) -> usize;
}

salsa::query_prototype! {
    crate trait Database: Counter {
        fn memoized(key: ()) -> usize {
            type Memoized;
        }
        fn volatile(key: ()) -> usize {
            type Volatile;
            storage volatile;
        }
    }
}

/// Because this query is memoized, we only increment the counter
/// the first time it is invoked.
impl<DB: Database> salsa::QueryFunction<DB> for Memoized {
    fn execute(db: &DB, (): ()) -> usize {
        db.increment()
    }
}

/// Because this query is volatile, each time it is invoked,
/// we will increment the counter.
impl<DB: Database> salsa::QueryFunction<DB> for Volatile {
    fn execute(db: &DB, (): ()) -> usize {
        db.increment()
    }
}
