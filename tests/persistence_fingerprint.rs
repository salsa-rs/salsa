#![cfg(all(feature = "persistence", feature = "inventory"))]

use std::sync::atomic::{AtomicUsize, Ordering};

use salsa::{Database, DatabaseImpl, Setter};

#[salsa::input(persist)]
struct Source {
    value: u32,
}

// An intermediate result with no serde implementation.
struct Ast(u32);

fn fingerprint(ast: &Ast) -> u128 {
    u128::MAX - u128::from(ast.0)
}

static PARSES: AtomicUsize = AtomicUsize::new(0);
static READS: AtomicUsize = AtomicUsize::new(0);

#[salsa::tracked(persist(fingerprint = fingerprint))]
fn parse(db: &dyn Database, source: Source) -> Ast {
    PARSES.fetch_add(1, Ordering::SeqCst);
    Ast(source.value(db) % 2)
}

#[salsa::tracked(persist, returns(copy))]
fn public_type(db: &dyn Database, source: Source) -> u32 {
    READS.fetch_add(1, Ordering::SeqCst);
    parse(db, source).0
}

fn restart(mut db: DatabaseImpl) -> DatabaseImpl {
    let saved = serde_json::to_vec(&<dyn Database>::as_serialize(&mut db)).unwrap();
    let mut db = DatabaseImpl::default();
    <dyn Database>::deserialize(&mut db, &mut serde_json::Deserializer::from_slice(&saved))
        .unwrap();
    db
}

#[test]
fn validate_and_restore_a_fingerprint() {
    let db = DatabaseImpl::default();
    let source = Source::new(&db, 1);
    assert_eq!(public_type(&db, source), 1);

    let mut db = restart(restart(db));
    assert_eq!(public_type(&db, source), 1);
    assert_eq!(PARSES.load(Ordering::SeqCst), 1);

    source.set_value(&mut db).to(3);
    assert_eq!(public_type(&db, source), 1);
    assert_eq!(PARSES.load(Ordering::SeqCst), 2);
    assert_eq!(READS.load(Ordering::SeqCst), 1);

    let mut db = restart(db);
    assert_eq!(parse(&db, source).0, 1);
    assert_eq!(PARSES.load(Ordering::SeqCst), 3);
    source.set_value(&mut db).to(4);
    assert_eq!(public_type(&db, source), 0);
    assert_eq!(READS.load(Ordering::SeqCst), 2);
}
