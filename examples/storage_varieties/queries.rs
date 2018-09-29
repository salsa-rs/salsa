crate trait CounterContext: salsa::QueryContext {
    fn increment(&self) -> usize;
}

crate trait QueryContext: CounterContext {
    salsa::query_prototype! {
        fn memoized() for Memoized;
        fn transparent() for Transparent;
        fn cycle_memoized() for CycleMemoized;
        fn cycle_transparent() for CycleTransparent;
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
    /// Because this query is transparent, each time it is invoked,
    /// we will increment the counter.
    #[storage(transparent)]
    crate Transparent(query: &impl QueryContext, (): ()) -> usize {
        query.increment()
    }
}

salsa::query_definition! {
    /// Because this query is memoized, we only increment the counter
    /// the first time it is invoked.
    crate CycleMemoized(query: &impl QueryContext, (): ()) -> usize {
        query.cycle_memoized().of(())
    }
}

salsa::query_definition! {
    /// Because this query is transparent, each time it is invoked,
    /// we will increment the counter.
    #[storage(transparent)]
    crate CycleTransparent(query: &impl QueryContext, (): ()) -> usize {
        query.cycle_transparent().of(())
    }
}
