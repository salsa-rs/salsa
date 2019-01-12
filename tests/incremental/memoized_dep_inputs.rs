use crate::implementation::{TestContext, TestContextImpl};
use salsa::Database;

#[salsa::query_group]
pub(crate) trait MemoizedDepInputsContext: TestContext {
    fn dep_memoized2(&self) -> usize;
    fn dep_memoized1(&self) -> usize;
    #[salsa::dependencies]
    fn dep_derived1(&self) -> usize;
    #[salsa::input]
    fn dep_input1(&self) -> usize;
    #[salsa::input]
    fn dep_input2(&self) -> usize;
}

fn dep_memoized2(db: &impl MemoizedDepInputsContext) -> usize {
    db.log().add("Memoized2 invoked");
    db.dep_memoized1()
}

fn dep_memoized1(db: &impl MemoizedDepInputsContext) -> usize {
    db.log().add("Memoized1 invoked");
    db.dep_derived1() * 2
}

fn dep_derived1(db: &impl MemoizedDepInputsContext) -> usize {
    db.log().add("Derived1 invoked");
    db.dep_input1() / 2
}

#[test]
fn revalidate() {
    let db = &mut TestContextImpl::default();

    db.query_mut(DepInput1Query).set((), 0);

    // Initial run starts from Memoized2:
    let v = db.dep_memoized2();
    assert_eq!(v, 0);
    db.assert_log(&["Memoized2 invoked", "Memoized1 invoked", "Derived1 invoked"]);

    // After that, we first try to validate Memoized1 but wind up
    // running Memoized2. Note that we don't try to validate
    // Derived1, so it is invoked by Memoized1.
    db.query_mut(DepInput1Query).set((), 44);
    let v = db.dep_memoized2();
    assert_eq!(v, 44);
    db.assert_log(&["Memoized1 invoked", "Derived1 invoked", "Memoized2 invoked"]);

    // Here validation of Memoized1 succeeds so Memoized2 never runs.
    db.query_mut(DepInput1Query).set((), 45);
    let v = db.dep_memoized2();
    assert_eq!(v, 44);
    db.assert_log(&["Memoized1 invoked", "Derived1 invoked"]);

    // Here, a change to input2 doesn't affect us, so nothing runs.
    db.query_mut(DepInput2Query).set((), 45);
    let v = db.dep_memoized2();
    assert_eq!(v, 44);
    db.assert_log(&[]);
}
