crate trait CounterContext: salsa::BaseQueryContext {
    fn increment(&self) -> usize;
}

crate trait QueryContext: CounterContext {
    salsa::query_prototype! {
        fn memoized() for Memoized;
        fn transparent() for Transparent;
    }
}

salsa::query_definition! {
    crate Memoized(query: &impl QueryContext, (): ()) -> usize {
        query.increment()
    }
}

salsa::query_definition! {
    #[storage(transparent)]
    crate Transparent(query: &impl QueryContext, (): ()) -> usize {
        query.increment()
    }
}
