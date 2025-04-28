use salsa::Database as Db;

#[salsa::input]
struct MyInput {}

#[derive(Clone, Debug)]
struct NotUpdate;

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn Db, input: MyInput) -> NotUpdate {
    _ = (db, input);
    NotUpdate
}

fn main() {}
