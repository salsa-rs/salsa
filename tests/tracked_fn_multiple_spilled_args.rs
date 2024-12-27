//! Tests that:
//! - a `tracked` fn with a single Salsa struct arg
//!   and a single, non-Salsa struct compiles and
//!   executes successfully.
//! - the size and number of allocations
//!   made by Salsa while executing a `tracked` fn
//!   with a `salsa::input` and non-Salsa struct.

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
fn tracked_fn<'db>(db: &'db dyn salsa::Database, input: MyInput, id: u32) -> u32 {
    input.field(db) + id
}

#[test]
fn execute() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    let interned = 33;
    assert_eq!(tracked_fn(&db, input, interned), 55);

    let stats = dhat::HeapStats::get();
    dhat::assert_eq!(stats.total_blocks, 31);
    dhat::assert_eq!(stats.total_bytes, 177224);
}
