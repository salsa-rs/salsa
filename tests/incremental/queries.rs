use crate::counter::Counter;
use crate::log::Log;

crate trait CounterContext: salsa::QueryContext {
    fn clock(&self) -> &Counter;
    fn log(&self) -> &Log;
}

crate trait QueryContext: CounterContext {
    salsa::query_prototype! {
        fn memoized2() for Memoized2;
        fn memoized1() for Memoized1;
        fn volatile() for Volatile;
    }
}

salsa::query_definition! {
    crate Memoized2(query: &impl QueryContext, (): ()) -> usize {
        query.log().add("Memoized2 invoked");
        query.memoized1().of(())
    }
}

salsa::query_definition! {
    crate Memoized1(query: &impl QueryContext, (): ()) -> usize {
        query.log().add("Memoized1 invoked");
        let v = query.volatile().of(());
        v / 2
    }
}

salsa::query_definition! {
    #[storage(volatile)]
    crate Volatile(query: &impl QueryContext, (): ()) -> usize {
        query.log().add("Volatile invoked");
        query.clock().increment()
    }
}
