//! Tests that:
//! - a `tracked` fn on multiple salsa struct args
//!   compiles and executes successfully.
//! - the size and number of allocations
//!   made by Salsa while executing a `tracked` fn
//!   with a `salsa::input` and a `salsa::interned`.

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::interned]
struct MyInterned<'db> {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn<'db>(db: &'db dyn salsa::Database, input: MyInput, interned: MyInterned<'db>) -> u32 {
    input.field(db) + interned.field(db)
}

#[test]
fn execute() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    let interned = MyInterned::new(&db, 33);
    assert_eq!(tracked_fn(&db, input, interned), 55);

    let stats = dhat::HeapStats::get();
    dhat::assert_eq!(stats.total_blocks, 37);
    dhat::assert_eq!(stats.total_bytes, 259292);
}
