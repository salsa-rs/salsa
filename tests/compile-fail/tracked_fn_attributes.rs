#![deny(deprecated, unused_must_use)]

#[salsa::input]
struct Input {}

#[must_use = "use the query result"]
#[deprecated(note = "old query")]
#[salsa::tracked(returns(copy))]
fn query(_db: &dyn salsa::Database) -> u32 {
    1
}

#[salsa::tracked]
impl Input {
    #[salsa::tracked(returns(copy))]
    #[must_use = "use the method result"]
    #[deprecated(note = "old method")]
    fn method(self, _db: &dyn salsa::Database) -> u32 {
        2
    }

    #[salsa::tracked(returns(copy))]
    #[cfg_attr(
        all(),
        must_use = "use the associated function result",
        deprecated(note = "old associated function")
    )]
    fn associated(_db: &dyn salsa::Database) -> u32 {
        3
    }
}

fn main() {
    let db = salsa::DatabaseImpl::new();
    let input = Input::new(&db);
    query(&db);
    input.method(&db);
    Input::associated(&db);
}
