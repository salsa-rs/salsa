use crate::implementation::{TestContext, TestContextImpl};
use salsa::Query;

salsa::query_prototype! {
    crate trait MemoizedInputsContext: TestContext {
        fn max(key: ()) -> usize {
            type Max;
        }
        fn input1(key: ()) -> usize {
            type Input1;
        }
        fn input2(key: ()) -> usize {
            type Input2;
        }
    }
}

salsa::query_definition! {
    crate Max(db: &impl MemoizedInputsContext, (): ()) -> usize {
        db.log().add("Max invoked");
        std::cmp::max(
            db.input1(()),
            db.input2(()),
        )
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

    let v = db.max(());
    assert_eq!(v, 0);
    db.assert_log(&["Max invoked"]);

    let v = db.max(());
    assert_eq!(v, 0);
    db.assert_log(&[]);

    Input1.set(db, (), 44);
    db.assert_log(&[]);

    let v = db.max(());
    assert_eq!(v, 44);
    db.assert_log(&["Max invoked"]);

    let v = db.max(());
    assert_eq!(v, 44);
    db.assert_log(&[]);

    Input1.set(db, (), 44);
    db.assert_log(&[]);
    Input2.set(db, (), 66);
    db.assert_log(&[]);
    Input1.set(db, (), 64);
    db.assert_log(&[]);

    let v = db.max(());
    assert_eq!(v, 66);
    db.assert_log(&["Max invoked"]);

    let v = db.max(());
    assert_eq!(v, 66);
    db.assert_log(&[]);
}
