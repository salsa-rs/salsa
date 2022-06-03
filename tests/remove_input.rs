//! Test that transparent (uncached) queries work

#[salsa::query_group(QueryGroupStorage)]
trait QueryGroup {
    #[salsa::input]
    fn input(&self, c: char) -> u32;

    fn increment(&self, c: char) -> u32;
}

fn increment(db: &dyn QueryGroup, c: char) -> u32 {
    db.input(c) + 1
}

#[salsa::database(QueryGroupStorage)]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

impl salsa::Database for Database {}

#[test]
fn remove_input_from_cached_query() {
    let mut db = Database::default();

    db.set_input('a', 22);
    db.set_input('b', 44);
    assert_eq!(db.increment('a'), 23);
    assert_eq!(db.increment('b'), 45);

    db.remove_input('a');
    assert_eq!(db.increment('b'), 45);
}

#[test]
fn remove_and_restore_input_from_cached_query() {
    let mut db = Database::default();

    db.set_input('a', 22);
    db.set_input('b', 44);
    assert_eq!(db.increment('a'), 23);
    assert_eq!(db.increment('b'), 45);

    db.remove_input('a');
    db.set_input('a', 66);
    assert_eq!(db.increment('a'), 67);
}
