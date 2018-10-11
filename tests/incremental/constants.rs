use crate::implementation::{TestContext, TestContextImpl};
use salsa::debug::DebugQueryTable;
use salsa::Database;

salsa::query_group! {
    pub(crate) trait ConstantsDatabase: TestContext {
        fn constants_input(key: char) -> usize {
            type ConstantsInput;
            storage input;
        }

        fn constants_add(keys: (char, char)) -> usize {
            type ConstantsAdd;
        }
    }
}

fn constants_add(db: &impl ConstantsDatabase, (key1, key2): (char, char)) -> usize {
    db.log()
        .add(format!("constants_derived({}, {}) invoked", key1, key2));
    db.constants_input(key1) + db.constants_input(key2)
}

#[test]
#[should_panic]
fn invalidate_constant() {
    let db = &TestContextImpl::default();
    db.query(ConstantsInput).set_constant('a', 44);
    db.query(ConstantsInput).set_constant('a', 66);
}

#[test]
#[should_panic]
fn invalidate_constant_1() {
    let db = &TestContextImpl::default();

    // Not constant:
    db.query(ConstantsInput).set('a', 44);

    // Becomes constant:
    db.query(ConstantsInput).set_constant('a', 44);

    // Invalidates:
    db.query(ConstantsInput).set_constant('a', 66);
}

#[test]
fn not_constant() {
    let db = &TestContextImpl::default();

    db.query(ConstantsInput).set('a', 22);
    db.query(ConstantsInput).set('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(!db.query(ConstantsAdd).is_constant(('a', 'b')));
}

#[test]
fn is_constant() {
    let db = &TestContextImpl::default();

    db.query(ConstantsInput).set_constant('a', 22);
    db.query(ConstantsInput).set_constant('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(db.query(ConstantsAdd).is_constant(('a', 'b')));
}

#[test]
fn mixed_constant() {
    let db = &TestContextImpl::default();

    db.query(ConstantsInput).set_constant('a', 22);
    db.query(ConstantsInput).set('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(!db.query(ConstantsAdd).is_constant(('a', 'b')));
}

#[test]
fn becomes_constant() {
    let db = &TestContextImpl::default();

    db.query(ConstantsInput).set('a', 22);
    db.query(ConstantsInput).set('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(!db.query(ConstantsAdd).is_constant(('a', 'b')));

    db.query(ConstantsInput).set_constant('a', 23);
    assert_eq!(db.constants_add(('a', 'b')), 67);
    assert!(!db.query(ConstantsAdd).is_constant(('a', 'b')));

    db.query(ConstantsInput).set_constant('b', 45);
    assert_eq!(db.constants_add(('a', 'b')), 68);
    assert!(db.query(ConstantsAdd).is_constant(('a', 'b')));
}
