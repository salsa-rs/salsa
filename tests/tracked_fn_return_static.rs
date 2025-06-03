//! Test that a `tracked` function can return a
//! non-`Update` type, so long as it has `'static` lifetime.

use salsa::Database;

#[salsa::input]
struct Input {}

#[derive(Clone, PartialEq)]
struct NotUpdate<'a> {
    _marker: std::marker::PhantomData<&'a ()>,
}

#[salsa::tracked]
fn test(_db: &dyn salsa::Database, _: Input) -> NotUpdate<'static> {
    NotUpdate {
        _marker: std::marker::PhantomData,
    }
}

#[test]
fn invoke() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = Input::new(db);
        test(db, input);
    })
}
