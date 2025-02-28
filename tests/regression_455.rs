use salsa::{Database, Setter};

#[salsa::tracked]
fn memoized(db: &dyn Database, input: MyInput) -> u32 {
    memoized_a(db, MyTracked::new(db, input.field(db)))
}

#[salsa::tracked]
fn memoized_a<'db>(db: &'db dyn Database, tracked: MyTracked<'db>) -> u32 {
    MyTracked::new(db, 0);
    memoized_b(db, tracked)
}

fn recovery_fn<'db>(_db: &'db dyn Database, _cycle: &salsa::Cycle, _input: MyTracked<'db>) -> u32 {
    0
}

#[salsa::tracked(recovery_fn=recovery_fn)]
fn memoized_b<'db>(db: &'db dyn Database, tracked: MyTracked<'db>) -> u32 {
    tracked.field(db) + memoized_a(db, tracked)
}

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: u32,
}

#[test]
fn cycle_memoized() {
    let mut db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 2);
    memoized(&db, input);
    input.set_field(&mut db).to(3);
    memoized(&db, input);
}
