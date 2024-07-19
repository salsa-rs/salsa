//! Compile Singleton struct test:
//!
//! Singleton flags are only allowed for input structs. If applied on any other Salsa struct compilation must fail

#[salsa::input(singleton)]
struct MyInput {
    field: u32,
}

#[salsa::tracked(singleton)]
struct MyTracked<'db> {
    field: u32,
}

#[salsa::tracked(singleton)]
fn create_tracked_structs(db: &dyn salsa::Database, input: MyInput) -> Vec<MyTracked> {
    (0..input.field(db))
        .map(|i| MyTracked::new(db, i))
        .collect()
}

#[salsa::accumulator(singleton)]
struct Integers(u32);

fn main() {}
