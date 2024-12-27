//! Tests that:
//! - `tracked` fn on a `salsa::input`
//!   compiles and executes successfully.
//! - The size and number of allocations
//!   made by Salsa while executing a `tracked` fn
//!   with a `salsa::input`.
#![allow(warnings)]

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
fn tracked_fn(db: &dyn salsa::Database, input: MyInput) -> u32 {
    input.field(db) * 2
}

#[test]
fn execute() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let mut db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    assert_eq!(tracked_fn(&db, input), 44);

    let stats = dhat::HeapStats::get();
    dhat::assert_eq!(stats.total_blocks, 26);
    dhat::assert_eq!(stats.total_bytes, 95164);
}
