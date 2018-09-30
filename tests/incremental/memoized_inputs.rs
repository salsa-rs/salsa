use crate::implementation::{TestContext, TestContextImpl};

crate trait MemoizedInputsContext: TestContext {
    salsa::query_prototype! {
        fn max() for Max;
        fn input1() for Input1;
        fn input2() for Input2;
    }
}

salsa::query_definition! {
    crate Max(query: &impl MemoizedInputsContext, (): ()) -> usize {
        query.log().add("Max invoked");
        std::cmp::max(
            query.input1().read(),
            query.input2().read(),
        )
    }
}

salsa::query_definition! {
    #[storage(input)]
    crate Input1(_query: &impl MemoizedInputsContext, _value: ()) -> usize {
        panic!("silly")
    }
}

salsa::query_definition! {
    #[storage(input)]
    crate Input2(_query: &impl MemoizedInputsContext, _value: ()) -> usize {
        panic!("silly")
    }
}

#[test]
fn revalidate() {
    let query = TestContextImpl::default();

    let v = query.max().of(());
    assert_eq!(v, 0);
    query.assert_log(&["Max invoked"]);

    let v = query.max().of(());
    assert_eq!(v, 0);
    query.assert_log(&[]);

    query.input1().set((), 44);
    query.assert_log(&[]);

    let v = query.max().of(());
    assert_eq!(v, 44);
    query.assert_log(&["Max invoked"]);

    let v = query.max().of(());
    assert_eq!(v, 44);
    query.assert_log(&[]);

    query.input1().set((), 44);
    query.assert_log(&[]);
    query.input2().set((), 66);
    query.assert_log(&[]);
    query.input1().set((), 64);
    query.assert_log(&[]);

    let v = query.max().of(());
    assert_eq!(v, 66);
    query.assert_log(&["Max invoked"]);

    let v = query.max().of(());
    assert_eq!(v, 66);
    query.assert_log(&[]);
}
