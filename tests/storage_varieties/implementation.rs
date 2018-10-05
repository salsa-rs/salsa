use crate::queries;
use std::cell::Cell;

#[derive(Default)]
pub struct DatabaseImpl {
    runtime: salsa::runtime::Runtime<DatabaseImpl>,
    counter: Cell<usize>,
}

salsa::database_storage! {
    pub struct DatabaseImplStorage for DatabaseImpl {
        impl queries::Database {
            fn memoized() for queries::Memoized;
            fn volatile() for queries::Volatile;
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
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<DatabaseImpl> {
        &self.runtime
    }
}
