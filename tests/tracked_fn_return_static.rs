//! Test that a `tracked` function can return a
//! non-`Update` type, so long as it has `'static` lifetime.

use salsa::Database;

#[salsa::input]
struct Input {}

#[salsa::tracked]
fn test(_db: &dyn salsa::Database, _: Input) -> &'static str {
    "test"
}

#[test]
fn invoke() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = Input::new(db);
        let x: &str = test(db, input);
        assert_eq!(x, "test");
    })
}
