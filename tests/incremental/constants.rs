use crate::implementation::{TestContext, TestContextImpl};
use salsa::debug::DebugQueryTable;
use salsa::Database;

#[salsa::query_group(Constants)]
pub(crate) trait ConstantsDatabase: TestContext {
    #[salsa::input]
    fn input(&self, key: char) -> usize;

    fn add(&self, keys: (char, char)) -> usize;
}

fn add(db: &impl ConstantsDatabase, (key1, key2): (char, char)) -> usize {
    db.log().add(format!("add({}, {})", key1, key2));
    db.input(key1) + db.input(key2)
}

#[test]
#[should_panic]
fn invalidate_constant() {
    let db = &mut TestContextImpl::default();
    db.set_constant_input('a', 44);
    db.set_constant_input('a', 66);
}

#[test]
#[should_panic]
fn invalidate_constant_1() {
    let db = &mut TestContextImpl::default();

    // Not constant:
    db.set_input('a', 44);

    // Becomes constant:
    db.set_constant_input('a', 44);

    // Invalidates:
    db.set_constant_input('a', 66);
}

/// Test that invoking `set` on a constant is an error, even if you
/// don't change the value.
#[test]
#[should_panic]
fn set_after_constant_same_value() {
    let db = &mut TestContextImpl::default();
    db.set_constant_input('a', 44);
    db.set_input('a', 44);
}

#[test]
fn not_constant() {
    let db = &mut TestContextImpl::default();

    db.set_input('a', 22);
    db.set_input('b', 44);
    assert_eq!(db.add(('a', 'b')), 66);
    assert!(!db.query(AddQuery).is_constant(('a', 'b')));
}

#[test]
fn is_constant() {
    let db = &mut TestContextImpl::default();

    db.set_constant_input('a', 22);
    db.set_constant_input('b', 44);
    assert_eq!(db.add(('a', 'b')), 66);
    assert!(db.query(AddQuery).is_constant(('a', 'b')));
}

#[test]
fn mixed_constant() {
    let db = &mut TestContextImpl::default();

    db.set_constant_input('a', 22);
    db.set_input('b', 44);
    assert_eq!(db.add(('a', 'b')), 66);
    assert!(!db.query(AddQuery).is_constant(('a', 'b')));
}

#[test]
fn becomes_constant_with_change() {
    let db = &mut TestContextImpl::default();

    db.set_input('a', 22);
    db.set_input('b', 44);
    assert_eq!(db.add(('a', 'b')), 66);
    assert!(!db.query(AddQuery).is_constant(('a', 'b')));

    db.set_constant_input('a', 23);
    assert_eq!(db.add(('a', 'b')), 67);
    assert!(!db.query(AddQuery).is_constant(('a', 'b')));

    db.set_constant_input('b', 45);
    assert_eq!(db.add(('a', 'b')), 68);
    assert!(db.query(AddQuery).is_constant(('a', 'b')));
}
