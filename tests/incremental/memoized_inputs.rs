use crate::implementation::{TestContext, TestContextImpl};
use salsa::Query;

salsa::query_prototype! {
    crate trait MemoizedInputsContext: TestContext {
        fn max(key: ()) -> usize {
            type Max;
        }
        fn input1(key: ()) -> usize {
            type Input1;
            storage input;
        }
        fn input2(key: ()) -> usize {
            type Input2;
            storage input;
        }
    }
}

impl<DB: MemoizedInputsContext> salsa::QueryFunction<DB> for Max {
    fn execute(db: &DB, (): ()) -> usize {
        db.log().add("Max invoked");
        std::cmp::max(db.input1(()), db.input2(()))
    }
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
