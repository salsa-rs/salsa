use crate::implementation::{TestContext, TestContextImpl};
use salsa::debug::DebugQueryTable;
use salsa::Database;

#[salsa::query_group]
pub(crate) trait ConstantsDatabase: TestContext {
    #[salsa::input]
    fn constants_input(&self, key: char) -> usize;

    fn constants_add(&self, keys: (char, char)) -> usize;
}

fn constants_add(db: &impl ConstantsDatabase, (key1, key2): (char, char)) -> usize {
    db.log().add(format!("add({}, {})", key1, key2));
    db.constants_input(key1) + db.constants_input(key2)
}

#[test]
#[should_panic]
fn invalidate_constant() {
    let db = &mut TestContextImpl::default();
    db.query_mut(ConstantsInputQuery).set_constant('a', 44);
    db.query_mut(ConstantsInputQuery).set_constant('a', 66);
}

#[test]
#[should_panic]
fn invalidate_constant_1() {
    let db = &mut TestContextImpl::default();

    // Not constant:
    db.query_mut(ConstantsInputQuery).set('a', 44);

    // Becomes constant:
    db.query_mut(ConstantsInputQuery).set_constant('a', 44);

    // Invalidates:
    db.query_mut(ConstantsInputQuery).set_constant('a', 66);
}

/// Test that invoking `set` on a constant is an error, even if you
/// don't change the value.
#[test]
#[should_panic]
fn set_after_constant_same_value() {
    let db = &mut TestContextImpl::default();
    db.query_mut(ConstantsInputQuery).set_constant('a', 44);
    db.query_mut(ConstantsInputQuery).set('a', 44);
}

#[test]
fn not_constant() {
    let db = &mut TestContextImpl::default();

    db.query_mut(ConstantsInputQuery).set('a', 22);
    db.query_mut(ConstantsInputQuery).set('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(!db.query(ConstantsAddQuery).is_constant(('a', 'b')));
}

#[test]
fn is_constant() {
    let db = &mut TestContextImpl::default();

    db.query_mut(ConstantsInputQuery).set_constant('a', 22);
    db.query_mut(ConstantsInputQuery).set_constant('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(db.query(ConstantsAddQuery).is_constant(('a', 'b')));
}

#[test]
fn mixed_constant() {
    let db = &mut TestContextImpl::default();

    db.query_mut(ConstantsInputQuery).set_constant('a', 22);
    db.query_mut(ConstantsInputQuery).set('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(!db.query(ConstantsAddQuery).is_constant(('a', 'b')));
}

#[test]
fn becomes_constant_with_change() {
    let db = &mut TestContextImpl::default();

    db.query_mut(ConstantsInputQuery).set('a', 22);
    db.query_mut(ConstantsInputQuery).set('b', 44);
    assert_eq!(db.constants_add(('a', 'b')), 66);
    assert!(!db.query(ConstantsAddQuery).is_constant(('a', 'b')));

    db.query_mut(ConstantsInputQuery).set_constant('a', 23);
    assert_eq!(db.constants_add(('a', 'b')), 67);
    assert!(!db.query(ConstantsAddQuery).is_constant(('a', 'b')));

    db.query_mut(ConstantsInputQuery).set_constant('b', 45);
    assert_eq!(db.constants_add(('a', 'b')), 68);
    assert!(db.query(ConstantsAddQuery).is_constant(('a', 'b')));
}
