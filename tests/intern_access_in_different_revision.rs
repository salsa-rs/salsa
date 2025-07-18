#![cfg(feature = "inventory")]

use salsa::{Durability, Setter};

#[salsa::interned(no_lifetime)]
struct Interned {
    field: u32,
}

#[salsa::input]
struct Input {
    field: i32,
}

#[test]
fn the_test() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::builder(-123456)
        .field_durability(Durability::HIGH)
        .new(&db);
    // Create an intern in an early revision.
    let interned = Interned::new(&db, 0xDEADBEEF);
    // Trigger a new revision.
    input
        .set_field(&mut db)
        .with_durability(Durability::HIGH)
        .to(123456);
    // Read the interned value
    let _ = interned.field(&db);
}
