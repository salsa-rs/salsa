use crate::queries;
use std::cell::Cell;

#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::Runtime<QueryContextImpl>,
    storage: QueryContextImplStorage,
    counter: Cell<usize>,
}

salsa::query_context_storage! {
    struct QueryContextImplStorage for storage in QueryContextImpl {
        impl queries::QueryContext {
            fn memoized() for queries::Memoized;
            fn transparent() for queries::Transparent;
        }
    }
}

impl queries::CounterContext for QueryContextImpl {
    fn increment(&self) -> usize {
        let v = self.counter.get();
        self.counter.set(v + 1);
        v
    }
}

impl salsa::QueryContext for QueryContextImpl {
    type QueryDescriptor = salsa::dyn_descriptor::DynDescriptor;

    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextImpl> {
        &self.runtime
    }
}
