use crate::queries;
use std::cell::Cell;

#[derive(Default)]
pub struct QueryContextImpl {
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

impl salsa::BaseQueryContext for QueryContextImpl {
    type QueryDescriptor = salsa::dyn_descriptor::DynDescriptor;

    fn execute_query_implementation<Q>(
        &self,
        _descriptor: Self::QueryDescriptor,
        key: &Q::Key,
    ) -> Q::Value
    where
        Q: salsa::Query<Self>,
    {
        let value = Q::execute(self, key.clone());
        value
    }

    fn report_unexpected_cycle(&self, _descriptor: Self::QueryDescriptor) -> ! {
        panic!("cycle")
    }
}
