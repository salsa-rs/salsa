use crate::counter::Counter;
use crate::log::Log;
use crate::queries;

#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::runtime::Runtime<QueryContextImpl>,
    clock: Counter,
    log: Log,
}

salsa::query_context_storage! {
    pub struct QueryContextImplStorage for QueryContextImpl {
        impl queries::QueryContext {
            fn memoized2() for queries::Memoized2;
            fn memoized1() for queries::Memoized1;
            fn volatile() for queries::Volatile;
        }
    }
}

impl queries::CounterContext for QueryContextImpl {
    fn clock(&self) -> &Counter {
        &self.clock
    }

    fn log(&self) -> &Log {
        &self.log
    }
}

impl salsa::QueryContext for QueryContextImpl {
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextImpl> {
        &self.runtime
    }
}
