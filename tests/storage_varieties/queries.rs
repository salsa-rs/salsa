pub(crate) trait Counter: salsa::Database {
    fn increment(&self) -> usize;
}

#[salsa::query_group(GroupStruct)]
pub(crate) trait Database: Counter {
    fn memoized(&self) -> usize;
    fn volatile(&self) -> usize;
}

/// Because this query is memoized, we only increment the counter
/// the first time it is invoked.
fn memoized(db: &mut impl Database) -> usize {
    db.volatile()
}

/// Because this query is volatile, each time it is invoked,
/// we will increment the counter.
fn volatile(db: &mut impl Database) -> usize {
    db.salsa_runtime_mut().report_untracked_read();
    db.increment()
}
