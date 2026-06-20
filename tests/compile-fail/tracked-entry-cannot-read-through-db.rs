#[salsa::tracked]
struct Entity<'db> {
    value: u32,
}

fn main() {
    let mut db = salsa::DatabaseImpl::default();
    let entry = Entity::ingredient(&db)
        .entries(&mut db)
        .next()
        .unwrap();

    entry.as_struct().value(&db);
}
