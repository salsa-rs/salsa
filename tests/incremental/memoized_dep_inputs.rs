use crate::implementation::{TestContext, TestContextImpl};
use salsa::Query;

salsa::query_prototype! {
    crate trait MemoizedDepInputsContext: TestContext {
        fn dep_memoized2(key: ()) -> usize {
            type Memoized2;
        }
        fn dep_memoized1(key: ()) -> usize {
            type Memoized1;
        }
        fn dep_derived1(key: ()) -> usize {
            type Derived1;
        }
        fn dep_input1(key: ()) -> usize {
            type Input1;
        }
        fn dep_input2(key: ()) -> usize {
            type Input2;
        }
    }
}

salsa::query_definition! {
    crate Memoized2(db: &impl MemoizedDepInputsContext, (): ()) -> usize {
        db.log().add("Memoized2 invoked");
        db.dep_memoized1(())
    }
}

salsa::query_definition! {
    crate Memoized1(db: &impl MemoizedDepInputsContext, (): ()) -> usize {
        db.log().add("Memoized1 invoked");
        db.dep_derived1(()) * 2
    }
}

salsa::query_definition! {
    #[storage(dependencies)]
    crate Derived1(db: &impl MemoizedDepInputsContext, (): ()) -> usize {
        db.log().add("Derived1 invoked");
        db.dep_input1(()) / 2
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
    let db = &TestContextImpl::default();

    // Initial run starts from Memoized2:
    let v = db.dep_memoized2(());
    assert_eq!(v, 0);
    db.assert_log(&["Memoized2 invoked", "Memoized1 invoked", "Derived1 invoked"]);

    // After that, we first try to validate Memoized1 but wind up
    // running Memoized2. Note that we don't try to validate
    // Derived1, so it is invoked by Memoized1.
    Input1.set(db, (), 44);
    let v = db.dep_memoized2(());
    assert_eq!(v, 44);
    db.assert_log(&["Memoized1 invoked", "Derived1 invoked", "Memoized2 invoked"]);

    // Here validation of Memoized1 succeeds so Memoized2 never runs.
    Input1.set(db, (), 45);
    let v = db.dep_memoized2(());
    assert_eq!(v, 44);
    db.assert_log(&["Memoized1 invoked", "Derived1 invoked"]);

    // Here, a change to input2 doesn't affect us, so nothing runs.
    Input2.set(db, (), 45);
    let v = db.dep_memoized2(());
    assert_eq!(v, 44);
    db.assert_log(&[]);
}
