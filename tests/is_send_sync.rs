//! Test that a setting a field on a `#[salsa::input]`
//! overwrites and returns the old value.

use salsa::Database;
use test_log::test;

#[salsa::input]
struct MyInput {
    field: String,
}

#[salsa::tracked]
struct MyTracked<'db> {
    field: MyInterned<'db>,
}

#[salsa::interned]
struct MyInterned<'db> {
    field: String,
}

#[salsa::tracked]
fn test(db: &dyn Database, input: MyInput) {
    let input = is_send_sync(input);
    let interned = is_send_sync(MyInterned::new(db, input.field(db).clone()));
    let _tracked_struct = is_send_sync(MyTracked::new(db, interned));
}

fn is_send_sync<T: Send + Sync>(t: T) -> T {
    t
}

#[test]
fn execute() {
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, "Hello".to_string());
    test(&db, input);
}
