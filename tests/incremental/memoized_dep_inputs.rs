use crate::implementation::{TestContext, TestContextImpl};

salsa::query_prototype! {
    crate trait MemoizedDepInputsContext: TestContext {
        fn dep_memoized2() for Memoized2;
        fn dep_memoized1() for Memoized1;
        fn dep_derived1() for Derived1;
        fn dep_input1() for Input1;
        fn dep_input2() for Input2;
    }
}

salsa::query_definition! {
    crate Memoized2(db: &impl MemoizedDepInputsContext, (): ()) -> usize {
        db.log().add("Memoized2 invoked");
        db.dep_memoized1().read()
    }
}

salsa::query_definition! {
    crate Memoized1(db: &impl MemoizedDepInputsContext, (): ()) -> usize {
        db.log().add("Memoized1 invoked");
        db.dep_derived1().read() * 2
    }
}

salsa::query_definition! {
    #[storage(dependencies)]
    crate Derived1(db: &impl MemoizedDepInputsContext, (): ()) -> usize {
        db.log().add("Derived1 invoked");
        db.dep_input1().read() / 2
    }
}

salsa::query_definition! {
    crate Input1: Map<(), usize>;
}

salsa::query_definition! {
    crate Input2: Map<(), usize>;
}

#[test]
fn revalidate() {
    let query = TestContextImpl::default();

    // Initial run starts from Memoized2:
    let v = query.dep_memoized2().read();
    assert_eq!(v, 0);
    query.assert_log(&["Memoized2 invoked", "Memoized1 invoked", "Derived1 invoked"]);

    // After that, we first try to validate Memoized1 but wind up
    // running Memoized2. Note that we don't try to validate
    // Derived1, so it is invoked by Memoized1.
    query.dep_input1().set((), 44);
    let v = query.dep_memoized2().read();
    assert_eq!(v, 44);
    query.assert_log(&["Memoized1 invoked", "Derived1 invoked", "Memoized2 invoked"]);

    // Here validation of Memoized1 succeeds so Memoized2 never runs.
    query.dep_input1().set((), 45);
    let v = query.dep_memoized2().read();
    assert_eq!(v, 44);
    query.assert_log(&["Memoized1 invoked", "Derived1 invoked"]);

    // Here, a change to input2 doesn't affect us, so nothing runs.
    query.dep_input2().set((), 45);
    let v = query.dep_memoized2().read();
    assert_eq!(v, 44);
    query.assert_log(&[]);
}
