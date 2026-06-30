#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct StaticField;

#[salsa::interned]
struct MyInterned<'db> {
    field: StaticField,
}

fn main() {}
