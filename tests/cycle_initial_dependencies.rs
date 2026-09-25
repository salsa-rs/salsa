#![cfg(feature = "inventory")]

//! Regression test for a panic when reusing an interned value created by `cycle_initial`.
//!
//! Previously, the initializer's reads were recorded only on the calling query, leaving the
//! initial memo without dependencies. Other cycle heads reading that memo did not inherit
//! those dependencies. After an input change, a consumer could therefore reexecute with an
//! interned argument that had not been validated for the new revision, panicking when it
//! accessed the argument's fields.
//!
//! `head` first calls itself, so its initializer reads `input.value` and creates an interned
//! value before `reader` starts its own cycle. `reader` then reads `head`'s provisional value
//! and passes it to `consume`. When flattening `reader`'s dependencies, Salsa must include the
//! initializer's reads, including the dependency recorded by interning the value.
//!
//! The next revision changes only `input.revision`: the interned payload stays the same, but
//! `consume` must reexecute. Entering through `reader` is important because validating `head`
//! first could validate the interned value and hide the missing dependency. `consume` reads
//! `input.revision` before accessing the interned field, so validation discovers the change
//! and reexecutes it with its cached interned argument.
//!
//! The final assertion protects against this regression by checking that entering through
//! `reader` returns the unchanged payload without panicking on the interned field access.

use salsa::Setter;

#[salsa::input]
struct Input {
    #[returns(copy)]
    value: u32,
    #[returns(copy)]
    revision: u32,
}

#[salsa::interned]
struct Interned<'db> {
    #[returns(copy)]
    value: u32,
}

#[salsa::tracked(returns(copy), cycle_initial = initial)]
fn head(db: &dyn salsa::Database, input: Input) -> Interned<'_> {
    // The first read creates the cycle's initial value.
    let initial = head(db, input);
    reader(db, input);
    initial
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| 0)]
fn reader(db: &dyn salsa::Database, input: Input) -> u32 {
    // Start a subcycle before reading the head's initial value.
    reader(db, input);
    consume(db, input, head(db, input))
}

#[salsa::tracked(returns(copy))]
fn consume<'db>(db: &'db dyn salsa::Database, input: Input, value: Interned<'db>) -> u32 {
    // Reexecute during reader validation, before using the interned argument.
    input.revision(db);
    value.value(db)
}

fn initial(db: &dyn salsa::Database, _id: salsa::Id, input: Input) -> Interned<'_> {
    Interned::new(db, input.value(db))
}

#[test]
fn initial_value_is_validated_before_its_consumer() {
    let mut db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, 0, 0);
    head(&db, input);

    // The initial payload stays unchanged; only its consumer is invalidated.
    input.set_revision(&mut db).to(1);
    assert_eq!(reader(&db, input), 0);
}
