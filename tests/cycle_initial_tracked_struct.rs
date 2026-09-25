#![cfg(feature = "inventory")]

//! Cycle initializers cannot directly create tracked structs because replacing the initial memo
//! does not transfer ownership of those structs. This also applies to `cycle_result` when it
//! supplies the initial provisional value.
//!
//! An initializer can call another tracked query that creates and owns the struct. Check that
//! this preserves the struct's identity across revisions and updates its specified value.

use salsa::plumbing::AsId;
use salsa::{Database, Setter};

#[salsa::tracked]
struct Item<'db> {
    #[returns(copy)]
    value: u32,
}

fn initial(db: &dyn Database, _id: salsa::Id) -> Item<'_> {
    Item::new(db, 42)
}

#[salsa::tracked(returns(copy), cycle_initial = initial)]
fn cycle(db: &dyn Database) -> Item<'_> {
    cycle(db)
}

#[test]
#[should_panic(expected = "cycle initializers cannot directly create tracked structs")]
fn initializer_cannot_create_tracked_structs() {
    let db = salsa::DatabaseImpl::default();
    cycle(&db);
}

#[salsa::tracked(returns(copy), cycle_result = initial)]
fn fallback(db: &dyn Database) -> Item<'_> {
    fallback(db)
}

#[test]
#[should_panic(expected = "cycle initializers cannot directly create tracked structs")]
fn fallback_initializer_cannot_create_tracked_structs() {
    let db = salsa::DatabaseImpl::default();
    fallback(&db);
}

#[salsa::input]
struct Input {
    #[returns(copy)]
    revision: u32,
}

#[salsa::tracked(returns(copy), specify)]
fn specified(_db: &dyn Database, _item: Item<'_>) -> u32 {
    100
}

#[salsa::tracked(returns(copy))]
fn initial_item(db: &dyn Database, input: Input) -> Item<'_> {
    let revision = input.revision(db);
    let item = Item::new(db, 42);
    specified::specify(db, item, revision);
    item
}

#[salsa::tracked(returns(copy), cycle_initial = |db, _, input| initial_item(db, input))]
fn cycle_with_helper(db: &dyn Database, input: Input) -> Item<'_> {
    cycle_with_helper(db, input)
}

#[test]
fn initializer_can_return_tracked_structs_from_a_query() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 0);
    let first = cycle_with_helper(&db, input);
    let first_id = first.as_id();
    assert_eq!(first.value(&db), 42);
    assert_eq!(specified(&db, first), 0);

    input.set_revision(&mut db).to(1);
    let second = cycle_with_helper(&db, input);
    assert_eq!(second.as_id(), first_id);
    assert_eq!(second.value(&db), 42);
    assert_eq!(specified(&db, second), 1);
}
