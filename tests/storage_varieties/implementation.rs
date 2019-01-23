use crate::queries;
use std::cell::Cell;

#[derive(Default)]
pub(crate) struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
    counter: Cell<usize>,
}

salsa::database_storage! {
    pub(crate) struct DatabaseImplStorage for DatabaseImpl {
        impl queries::Database {
            fn memoized() for queries::MemoizedQuery;
            fn volatile() for queries::VolatileQuery;
        }
    }
}

impl queries::Counter for DatabaseImpl {
    fn increment(&self) -> usize {
        let v = self.counter.get();
        self.counter.set(v + 1);
        v
    }
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }
}
