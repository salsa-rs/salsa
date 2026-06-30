#[salsa::tracked]
struct MyInput<'db> {
    field: NotUpdate,
}

#[derive(Clone, Debug, Hash)]
struct NotUpdate;

fn main() {}
