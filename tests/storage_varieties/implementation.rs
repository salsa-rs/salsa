use crate::queries;
use crossbeam::atomic::AtomicCell;

#[salsa::database(queries::GroupStruct)]
#[derive(Default)]
pub(crate) struct DatabaseImpl {
    runtime: salsa::Runtime<DatabaseImpl>,
    counter: AtomicCell<usize>,
}

impl queries::Counter for DatabaseImpl {
    fn increment(&self) -> usize {
        self.counter.fetch_add(1)
    }
}

impl salsa::Database for DatabaseImpl {
    fn salsa_runtime(&self) -> &salsa::Runtime<DatabaseImpl> {
        &self.runtime
    }

    fn salsa_runtime_mut(&mut self) -> &mut salsa::Runtime<DatabaseImpl> {
        &mut self.runtime
    }
}
