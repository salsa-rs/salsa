#![cfg(feature = "inventory")]

//! Equality that ignores an interned value can leave a caller holding an unvalidated ID.

use salsa::plumbing::AsId;
use salsa::{Database, Setter};

#[salsa::input]
struct Input {
    #[returns(copy)]
    value: u32,
}

#[salsa::interned]
struct Value<'db> {
    #[returns(copy)]
    value: u32,
}

#[derive(Clone, Copy, Eq, salsa::SalsaValue)]
struct Wrapper<'db> {
    value: Value<'db>,
}

impl PartialEq for Wrapper<'_> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

#[test]
#[should_panic(expected = "was not interned in the latest revision for its durability")]
fn bad_equality_cannot_expose_unvalidated_interned_data() {
    let mut db = salsa::DatabaseImpl::new();
    let input = Input::new(&db, 0);
    let a = consumer(&db, input);
    assert_eq!(a.value(&db), 0);
    let a_id = a.as_id();

    // The producer now interns B instead of A, but the wrappers compare equal.
    input.set_value(&mut db).to(1);
    let b = producer(&db, input).value;
    assert_eq!(b.value(&db), 1);
    assert_ne!(b.as_id(), a_id);

    // Backdating preserves the consumer's cached A without validating A in this revision.
    let cached_a = consumer(&db, input);
    assert_eq!(cached_a.as_id(), a_id);
    cached_a.value(&db);
}

#[salsa::tracked(returns(copy))]
fn consumer<'db>(db: &'db dyn Database, input: Input) -> Value<'db> {
    producer(db, input).value
}

#[salsa::tracked(returns(copy))]
fn producer<'db>(db: &'db dyn Database, input: Input) -> Wrapper<'db> {
    Wrapper {
        value: Value::new(db, input.value(db)),
    }
}
