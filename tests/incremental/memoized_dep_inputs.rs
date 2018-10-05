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
            storage dependencies;
        }
        fn dep_input1(key: ()) -> usize {
            type Input1;
            storage input;
        }
        fn dep_input2(key: ()) -> usize {
            type Input2;
            storage input;
        }
    }
}

impl<DB: MemoizedDepInputsContext> salsa::QueryFunction<DB> for Memoized2 {
    fn execute(db: &DB, (): ()) -> usize {
        db.log().add("Memoized2 invoked");
        db.dep_memoized1(())
    }
}

impl<DB: MemoizedDepInputsContext> salsa::QueryFunction<DB> for Memoized1 {
    fn execute(db: &DB, (): ()) -> usize {
        db.log().add("Memoized1 invoked");
        db.dep_derived1(()) * 2
    }
}

impl<DB: MemoizedDepInputsContext> salsa::QueryFunction<DB> for Derived1 {
    fn execute(db: &DB, (): ()) -> usize {
        db.log().add("Derived1 invoked");
        db.dep_input1(()) / 2
    }
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
