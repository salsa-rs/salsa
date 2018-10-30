use crate::implementation::{TestContext, TestContextImpl};
use salsa::Database;

salsa::query_group! {
    pub(crate) trait MemoizedInputsContext: TestContext {
        fn max() -> usize {
            type Max;
        }
        fn input1() -> usize {
            type Input1;
            storage input;
        }
        fn input2() -> usize {
            type Input2;
            storage input;
        }
    }
}

fn max(db: &impl MemoizedInputsContext) -> usize {
    db.log().add("Max invoked");
    std::cmp::max(db.input1(), db.input2())
}

#[test]
fn revalidate() {
    let db = &TestContextImpl::default();

    db.query(Input1).set((), 0);
    db.query(Input2).set((), 0);

    let v = db.max();
    assert_eq!(v, 0);
    db.assert_log(&["Max invoked"]);

    let v = db.max();
    assert_eq!(v, 0);
    db.assert_log(&[]);

    db.query(Input1).set((), 44);
    db.assert_log(&[]);

    let v = db.max();
    assert_eq!(v, 44);
    db.assert_log(&["Max invoked"]);

    let v = db.max();
    assert_eq!(v, 44);
    db.assert_log(&[]);

    db.query(Input1).set((), 44);
    db.assert_log(&[]);
    db.query(Input2).set((), 66);
    db.assert_log(&[]);
    db.query(Input1).set((), 64);
    db.assert_log(&[]);

    let v = db.max();
    assert_eq!(v, 66);
    db.assert_log(&["Max invoked"]);

    let v = db.max();
    assert_eq!(v, 66);
    db.assert_log(&[]);
}

/// Test that invoking `set` on an input with the same value still
/// triggers a new revision.
#[test]
fn set_after_no_change() {
    let db = &TestContextImpl::default();

    db.query(Input2).set((), 0);

    db.query(Input1).set((), 44);
    let v = db.max();
    assert_eq!(v, 44);
    db.assert_log(&["Max invoked"]);

    db.query(Input1).set((), 44);
    let v = db.max();
    assert_eq!(v, 44);
    db.assert_log(&["Max invoked"]);
}
