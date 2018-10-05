use crate::implementation::{TestContext, TestContextImpl};

salsa::query_prototype! {
    crate trait MemoizedInputsContext: TestContext {
        fn max() for Max;
        fn input1() for Input1;
        fn input2() for Input2;
    }
}

salsa::query_definition! {
    crate Max(db: &impl MemoizedInputsContext, (): ()) -> usize {
        db.log().add("Max invoked");
        std::cmp::max(
            db.input1().read(),
            db.input2().read(),
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
    let query = TestContextImpl::default();

    let v = query.max().read();
    assert_eq!(v, 0);
    query.assert_log(&["Max invoked"]);

    let v = query.max().read();
    assert_eq!(v, 0);
    query.assert_log(&[]);

    query.input1().set((), 44);
    query.assert_log(&[]);

    let v = query.max().read();
    assert_eq!(v, 44);
    query.assert_log(&["Max invoked"]);

    let v = query.max().read();
    assert_eq!(v, 44);
    query.assert_log(&[]);

    query.input1().set((), 44);
    query.assert_log(&[]);
    query.input2().set((), 66);
    query.assert_log(&[]);
    query.input1().set((), 64);
    query.assert_log(&[]);

    let v = query.max().read();
    assert_eq!(v, 66);
    query.assert_log(&["Max invoked"]);

    let v = query.max().read();
    assert_eq!(v, 66);
    query.assert_log(&[]);
}
