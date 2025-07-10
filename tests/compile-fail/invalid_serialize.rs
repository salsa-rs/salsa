#[salsa::tracked(serialize)]
struct Serializable<'db> {
    field: NotSerializable<'db>,
}

#[salsa::tracked]
struct NotSerializable<'db> {
    field: usize,
}

#[salsa::tracked(serialize)]
fn query(_db: &dyn salsa::Database, _input: NotSerializable<'_>) {}

fn main() {}
