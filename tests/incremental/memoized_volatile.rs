use crate::implementation::{TestContext, TestContextImpl};
use salsa::QueryContext;

crate trait MemoizedVolatileContext: TestContext {
    salsa::query_prototype! {
        // Queries for testing a "volatile" value wrapped by
        // memoization.
        fn memoized2() for Memoized2;
        fn memoized1() for Memoized1;
        fn volatile() for Volatile;
    }
}

salsa::query_definition! {
    crate Memoized2(query: &impl MemoizedVolatileContext, (): ()) -> usize {
        query.log().add("Memoized2 invoked");
        query.memoized1().read()
    }
}

salsa::query_definition! {
    crate Memoized1(query: &impl MemoizedVolatileContext, (): ()) -> usize {
        query.log().add("Memoized1 invoked");
        let v = query.volatile().read();
        v / 2
    }
}

salsa::query_definition! {
    #[storage(volatile)]
    crate Volatile(query: &impl MemoizedVolatileContext, (): ()) -> usize {
        query.log().add("Volatile invoked");
        query.clock().increment()
    }
}

#[test]
fn volatile_x2() {
    let query = TestContextImpl::default();

    // Invoking volatile twice will simply execute twice.
    query.volatile().read();
    query.volatile().read();
    query.assert_log(&["Volatile invoked", "Volatile invoked"]);
}

/// Test that:
///
/// - On the first run of R0, we recompute everything.
/// - On the second run of R1, we recompute nothing.
/// - On the first run of R1, we recompute Memoized1 but not Memoized2 (since Memoized1 result
///   did not change).
/// - On the second run of R1, we recompute nothing.
/// - On the first run of R2, we recompute everything (since Memoized1 result *did* change).
#[test]
fn revalidate() {
    let query = TestContextImpl::default();

    query.memoized2().read();
    query.assert_log(&["Memoized2 invoked", "Memoized1 invoked", "Volatile invoked"]);

    query.memoized2().read();
    query.assert_log(&[]);

    // Second generation: volatile will change (to 1) but memoized1
    // will not (still 0, as 1/2 = 0)
    query.salsa_runtime().next_revision();

    query.memoized2().read();
    query.assert_log(&["Memoized1 invoked", "Volatile invoked"]);

    query.memoized2().read();
    query.assert_log(&[]);

    // Third generation: volatile will change (to 2) and memoized1
    // will too (to 1).  Therefore, after validating that Memoized1
    // changed, we now invoke Memoized2.
    query.salsa_runtime().next_revision();

    query.memoized2().read();
    query.assert_log(&["Memoized1 invoked", "Volatile invoked", "Memoized2 invoked"]);

    query.memoized2().read();
    query.assert_log(&[]);
}
