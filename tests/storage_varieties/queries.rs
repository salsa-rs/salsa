pub(crate) trait Counter: salsa::Database {
    fn increment(&self) -> usize;
}

salsa::query_group! {
    pub(crate) trait Database: Counter {
        fn memoized() -> usize {
            type Memoized;
        }
        fn volatile() -> usize {
            type Volatile;
            storage volatile;
        }
    }
}

/// Because this query is memoized, we only increment the counter
/// the first time it is invoked.
fn memoized(db: &impl Database) -> usize {
    db.volatile()
}

/// Because this query is volatile, each time it is invoked,
/// we will increment the counter.
fn volatile(db: &impl Database) -> usize {
    db.increment()
}
