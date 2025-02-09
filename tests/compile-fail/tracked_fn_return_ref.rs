use salsa::Database as Db;

#[salsa::input]
struct MyInput {
    #[return_ref]
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContainsRef<'db> {
    text: &'db str,
}

#[salsa::tracked]
fn tracked_fn_return_ref<'db>(db: &'db dyn Db, input: MyInput) -> &'db str {
    input.text(db)
}

#[salsa::tracked]
fn tracked_fn_return_struct_containing_ref<'db>(
    db: &'db dyn Db,
    input: MyInput,
) -> ContainsRef<'db> {
    ContainsRef {
        text: input.text(db),
    }
}

fn main() {}
