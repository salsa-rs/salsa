#![cfg(feature = "inventory")]

//! Reject an interned value whose dependency was lost when validation displaced its memo.

use salsa::{Database, Setter};

#[salsa::input]
struct Input {
    #[returns(copy)]
    enabled: bool,
}

#[salsa::interned]
struct Value<'db> {
    #[returns(copy)]
    value: u32,
}

// TODO: Fix validation of displaced memos and replace these expected panics with
// correctness assertions.
#[test]
#[should_panic(expected = "was not interned in the latest revision for its durability")]
fn displaced_memo_cannot_expose_unvalidated_interned_data() {
    let (db, input) = displaced_memo();
    let value = consumer(&db, input).unwrap();
    assert_eq!(value.value(&db), 1);
}

#[test]
#[should_panic(expected = "was not interned in the latest revision for its durability")]
fn displaced_memo_cannot_expose_unvalidated_interned_memos() {
    let (db, input) = displaced_memo();
    let value = consumer(&db, input).unwrap();
    // Borrowing a memo must check the key's validity even without reading its fields.
    payload(&db, value);
}

fn displaced_memo() -> (salsa::DatabaseImpl, Input) {
    let mut db = salsa::DatabaseImpl::new();
    let input = Input::new(&db, false);
    outer(&db, input);
    recursive_reader(&db, input);
    consumer(&db, input);

    // Verification forms a temporary cycle that displaces the producer's memo.
    input.set_enabled(&mut db).to(true);
    consumer(&db, input);
    producer(&db, input);

    // The cached consumer retains a value that the producer no longer validates.
    db.synthetic_write(salsa::Durability::LOW);
    (db, input)
}

#[salsa::tracked(returns(ref))]
fn payload<'db>(_db: &'db dyn Database, _value: Value<'db>) -> String {
    String::from("payload")
}

#[salsa::tracked(returns(copy))]
fn consumer<'db>(db: &'db dyn Database, input: Input) -> Option<Value<'db>> {
    producer(db, input)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| true)]
fn recursive_reader(db: &dyn Database, input: Input) -> bool {
    producer(db, input).is_some()
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| None)]
fn producer<'db>(db: &'db dyn Database, input: Input) -> Option<Value<'db>> {
    condition(db, input);
    let present = recursive_reader(db, input);
    // Both outcomes intern a value, so this does not depend on missing changed_at updates.
    let value = Value::new(db, u32::from(present));
    present.then_some(value)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| false)]
fn condition(db: &dyn Database, input: Input) -> bool {
    !outer(db, input)
}

#[salsa::tracked(returns(copy), cycle_initial = |_, _, _| false)]
fn outer(db: &dyn Database, input: Input) -> bool {
    if condition(db, input) {
        dependency(db, input);
    }
    true
}

#[salsa::tracked(returns(copy))]
fn dependency(db: &dyn Database, input: Input) {
    if input.enabled(db) {
        recursive_reader(db, input);
    }
}
