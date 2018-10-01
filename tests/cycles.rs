#![feature(crate_visibility_modifier)]

#[derive(Default)]
pub struct QueryContextImpl {
    runtime: salsa::runtime::Runtime<QueryContextImpl>,
}

impl salsa::QueryContext for QueryContextImpl {
    fn salsa_runtime(&self) -> &salsa::runtime::Runtime<QueryContextImpl> {
        &self.runtime
    }
}

salsa::query_context_storage! {
    pub struct QueryContextImplStorage for QueryContextImpl {
        impl QueryContext {
            fn memoized_a() for MemoizedA;
            fn memoized_b() for MemoizedB;
            fn volatile_a() for VolatileA;
            fn volatile_b() for VolatileB;
        }
    }
}

trait QueryContext: salsa::QueryContext {
    salsa::query_prototype! {
        // `a` and `b` depend on each other and form a cycle
        fn memoized_a() for MemoizedA;
        fn memoized_b() for MemoizedB;
        fn volatile_a() for VolatileA;
        fn volatile_b() for VolatileB;
    }
}

salsa::query_definition! {
    crate MemoizedA(query: &impl QueryContext, (): ()) -> () {
        query.memoized_b().get(())
    }
}

salsa::query_definition! {
    crate MemoizedB(query: &impl QueryContext, (): ()) -> () {
        query.memoized_a().get(())
    }
}

salsa::query_definition! {
    #[storage(volatile)]
    crate VolatileA(query: &impl QueryContext, (): ()) -> () {
        query.volatile_b().get(())
    }
}

salsa::query_definition! {
    #[storage(volatile)]
    crate VolatileB(query: &impl QueryContext, (): ()) -> () {
        query.volatile_a().get(())
    }
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_memoized() {
    let query = QueryContextImpl::default();
    query.memoized_a().get(());
}

#[test]
#[should_panic(expected = "cycle detected")]
fn cycle_volatile() {
    let query = QueryContextImpl::default();
    query.volatile_a().get(());
}
