crate trait CounterContext: salsa::QueryContext {
    fn increment(&self) -> usize;
}

crate trait QueryContext: CounterContext {
    salsa::query_prototype! {
        fn memoized() for Memoized;
        fn volatile() for Volatile;
    }
}

salsa::query_definition! {
    /// Because this query is memoized, we only increment the counter
    /// the first time it is invoked.
    crate Memoized(query: &impl QueryContext, (): ()) -> usize {
        query.increment()
    }
}

salsa::query_definition! {
    /// Because this query is volatile, each time it is invoked,
    /// we will increment the counter.
    #[storage(volatile)]
    crate Volatile(query: &impl QueryContext, (): ()) -> usize {
        query.increment()
    }
}
