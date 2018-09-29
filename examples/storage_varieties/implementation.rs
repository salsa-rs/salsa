use crate::queries;
use std::cell::Cell;

#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::runtime::Runtime<QueryContextImpl>,
    counter: Cell<usize>,
}

salsa::query_context_storage! {
    pub struct QueryContextImplStorage for QueryContextImpl {
        impl queries::QueryContext {
            fn memoized() for queries::Memoized;
            fn transparent() for queries::Transparent;
            fn cycle_memoized() for queries::CycleMemoized;
            fn cycle_transparent() for queries::CycleTransparent;
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
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextImpl> {
        &self.runtime
    }
}
