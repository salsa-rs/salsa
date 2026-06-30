use salsa::Database as Db;

#[derive(Clone, Debug, Hash, PartialEq, Eq, salsa::SalsaValue)]
struct StaticField;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StaticQueryResult;

#[salsa::input]
struct MyInput {
    field: StaticField,
}

#[salsa::interned]
struct MyInterned<'db> {
    field: StaticField,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: StaticField,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn Db, input: MyInput) -> StaticQueryResult {
    _ = input.field(db);
    StaticQueryResult
}

fn main() {}
