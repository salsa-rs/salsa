use crate::implementation::{TestContext, TestContextImpl};
use salsa::Database;

salsa::query_group! {
    pub(crate) trait ConstantsDatabase: TestContext {
        fn constants_input(key: usize) -> usize {
            type ConstantsInput;
            storage input;
        }

        fn constants_derived(key: usize) -> usize {
            type ConstantsDerived;
        }
    }
}

fn constants_derived(db: &impl ConstantsDatabase, key: usize) -> usize {
    db.log().add(format!("constants_derived({}) invoked", key));
    db.constants_input(key) * 2
}

#[test]
#[should_panic]
fn invalidate_constant() {
    let db = &TestContextImpl::default();
    db.query(ConstantsInput).set_constant(22, 44);
    db.query(ConstantsInput).set_constant(22, 66);
}

#[test]
#[should_panic]
fn invalidate_constant_1() {
    let db = &TestContextImpl::default();

    // Not constant:
    db.query(ConstantsInput).set(22, 44);

    // Becomes constant:
    db.query(ConstantsInput).set_constant(22, 44);

    // Invalidates:
    db.query(ConstantsInput).set_constant(22, 66);
}
