#[salsa::tracked]
pub struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked]
fn my_fn(db: &dyn salsa::Database) -> salsa::Result<()> {
    let x = MyTracked::new(db, 22)?;
    x.field(22)?;
    Ok(())
}

fn main() {
    let mut db = salsa::DatabaseImpl::new();
    my_fn(&db).unwrap();
}
