#![cfg(feature = "inventory")]

//! Tests for volatile tracked functions.

use std::sync::Arc;
#[cfg(feature = "salsa_unstable")]
use std::sync::Barrier;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "accumulator")]
use salsa::Accumulator;
use salsa::{Database as _, Durability};
use test_log::test;

const FILL_FIELDS: std::ops::Range<u32> = 100..1_124;

#[salsa::input]
struct MyInput {
    field: u32,
}

#[salsa::tracked]
struct CreatedByVolatile<'db> {
    field: u32,
}

#[derive(Clone, salsa::Update)]
struct TrackedReference<'db>(CreatedByVolatile<'db>);

#[salsa::tracked]
fn create_tracked(db: &dyn salsa::Database, input: MyInput) -> CreatedByVolatile<'_> {
    CreatedByVolatile::new(db, input.field(db))
}

thread_local! {
    static VOLATILE_EXECUTIONS: AtomicUsize = const { AtomicUsize::new(0) };
    static OUTER_EXECUTIONS: AtomicUsize = const { AtomicUsize::new(0) };
    static CYCLE_EXECUTIONS: AtomicUsize = const { AtomicUsize::new(0) };
    static TRACKED_EXECUTIONS: AtomicUsize = const { AtomicUsize::new(0) };
    static ACCUMULATE_EXECUTIONS: AtomicUsize = const { AtomicUsize::new(0) };
    static LIVE_VALUES: AtomicUsize = const { AtomicUsize::new(0) };
}

fn reset_counts() {
    VOLATILE_EXECUTIONS.with(|n| n.store(0, Ordering::SeqCst));
    OUTER_EXECUTIONS.with(|n| n.store(0, Ordering::SeqCst));
    CYCLE_EXECUTIONS.with(|n| n.store(0, Ordering::SeqCst));
    TRACKED_EXECUTIONS.with(|n| n.store(0, Ordering::SeqCst));
    ACCUMULATE_EXECUTIONS.with(|n| n.store(0, Ordering::SeqCst));
}

fn volatile_executions() -> usize {
    VOLATILE_EXECUTIONS.with(|n| n.load(Ordering::SeqCst))
}

fn outer_executions() -> usize {
    OUTER_EXECUTIONS.with(|n| n.load(Ordering::SeqCst))
}

fn cycle_executions() -> usize {
    CYCLE_EXECUTIONS.with(|n| n.load(Ordering::SeqCst))
}

#[cfg(feature = "accumulator")]
fn accumulate_executions() -> usize {
    ACCUMULATE_EXECUTIONS.with(|n| n.load(Ordering::SeqCst))
}

#[derive(PartialEq, Eq, salsa::Update)]
struct LiveValue;

impl LiveValue {
    fn new() -> Self {
        LIVE_VALUES.with(|n| n.fetch_add(1, Ordering::SeqCst));
        Self
    }
}

impl Drop for LiveValue {
    fn drop(&mut self) {
        LIVE_VALUES.with(|n| n.fetch_sub(1, Ordering::SeqCst));
    }
}

fn live_values() -> usize {
    LIVE_VALUES.with(|n| n.load(Ordering::SeqCst))
}

#[salsa::tracked(volatile = 2)]
fn volatile_value(db: &dyn salsa::Database, input: MyInput) -> u32 {
    VOLATILE_EXECUTIONS.with(|n| n.fetch_add(1, Ordering::SeqCst));
    input.field(db)
}

#[salsa::tracked(volatile = 2, returns(copy))]
fn volatile_copy(db: &dyn salsa::Database, input: MyInput) -> u32 {
    input.field(db)
}

#[salsa::tracked(volatile = 2)]
fn volatile_arc(_db: &dyn salsa::Database, _input: MyInput) -> Arc<LiveValue> {
    Arc::new(LiveValue::new())
}

#[salsa::tracked(volatile = 2, no_eq)]
fn volatile_tracked_reference<'db>(
    _db: &'db dyn salsa::Database,
    value: CreatedByVolatile<'db>,
) -> TrackedReference<'db> {
    TrackedReference(value)
}

#[salsa::tracked(volatile = 2, returns(copy))]
fn volatile_creates_tracked_struct(
    db: &dyn salsa::Database,
    input: MyInput,
) -> CreatedByVolatile<'_> {
    TRACKED_EXECUTIONS.with(|n| n.fetch_add(1, Ordering::SeqCst));
    CreatedByVolatile::new(db, input.field(db))
}

#[cfg(feature = "accumulator")]
#[salsa::accumulator]
struct VolatileLog(u32);

#[cfg(feature = "accumulator")]
#[salsa::tracked(volatile = 2)]
fn volatile_accumulate(db: &dyn salsa::Database, input: MyInput) -> u32 {
    ACCUMULATE_EXECUTIONS.with(|n| n.fetch_add(1, Ordering::SeqCst));
    let field = input.field(db);
    VolatileLog(field).accumulate(db);
    field
}

#[salsa::tracked]
fn outer_value(db: &dyn salsa::Database, input: MyInput) -> u32 {
    OUTER_EXECUTIONS.with(|n| n.fetch_add(1, Ordering::SeqCst));
    volatile_value(db, input) + 1
}

#[salsa::tracked(volatile = 2, cycle_initial=cycle_initial)]
fn volatile_cycle_value(db: &dyn salsa::Database, input: MyInput) -> u32 {
    CYCLE_EXECUTIONS.with(|n| n.fetch_add(1, Ordering::SeqCst));

    if input.field(db) != 0 {
        volatile_cycle_value(db, input);
    }

    input.field(db)
}

fn cycle_initial(_db: &dyn salsa::Database, _id: salsa::Id, _input: MyInput) -> u32 {
    0
}

fn fill_volatile_cache(db: &salsa::DatabaseImpl) {
    for field in FILL_FIELDS {
        let input = MyInput::new(db, field);
        assert_eq!(volatile_value(db, input), field);
    }
}

fn fill_volatile_cycle_cache(db: &salsa::DatabaseImpl) {
    for field in FILL_FIELDS {
        let input = MyInput::new(db, field);
        assert_eq!(volatile_cycle_value(db, input), field);
    }
}

#[test]
fn volatile_evicts_automatically_without_new_revision() {
    reset_counts();
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);

    assert_eq!(volatile_value(&db, input), 22);
    assert_eq!(volatile_value(&db, input), 22);
    assert_eq!(volatile_executions(), 1);

    fill_volatile_cache(&db);

    let executions_before_refetch = volatile_executions();
    assert_eq!(volatile_value(&db, input), 22);
    assert_eq!(volatile_executions(), executions_before_refetch + 1);
}

#[test]
fn volatile_supports_copy_return_mode() {
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);

    assert_eq!(volatile_copy(&db, input), 22);
}

#[test]
fn volatile_supports_non_static_update_output() {
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);
    let value = create_tracked(&db, input);

    assert_eq!(volatile_tracked_reference(&db, value).0.field(&db), 22);
}

#[test]
fn volatile_reuses_tracked_struct_ids_after_eviction() {
    reset_counts();
    let db = salsa::DatabaseImpl::new();
    let inputs = (0..3)
        .map(|field| MyInput::new(&db, field))
        .collect::<Vec<_>>();
    let original = inputs
        .iter()
        .map(|input| volatile_creates_tracked_struct(&db, *input))
        .collect::<Vec<_>>();

    for (input, expected) in inputs.iter().zip(original) {
        assert!(volatile_creates_tracked_struct(&db, *input) == expected);
    }

    TRACKED_EXECUTIONS.with(|n| assert!(n.load(Ordering::SeqCst) > 3));
}

#[test]
fn volatile_drops_values_without_new_revision() {
    assert_eq!(live_values(), 0);
    let db = salsa::DatabaseImpl::new();

    for field in 0..1_024 {
        let input = MyInput::new(&db, field);
        drop(volatile_arc(&db, input));
    }

    assert!(live_values() < 1_024);
}

#[cfg(feature = "accumulator")]
#[test]
fn volatile_keeps_escaped_accumulated_values_alive() {
    reset_counts();
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);

    let logs = volatile_accumulate::accumulated::<VolatileLog>(&db, input);
    assert_eq!(logs[0].0, 22);

    for field in FILL_FIELDS {
        let input = MyInput::new(&db, field);
        assert_eq!(volatile_accumulate(&db, input), field);
    }

    let executions_before_refetch = accumulate_executions();
    assert_eq!(volatile_accumulate(&db, input), 22);
    assert_eq!(accumulate_executions(), executions_before_refetch);
    assert_eq!(logs[0].0, 22);
}

#[test]
fn volatile_eviction_is_safe_with_parallel_reads() {
    let db = salsa::DatabaseImpl::new();
    let inputs = (0..256)
        .map(|field| MyInput::new(&db, field))
        .collect::<Vec<_>>();

    let threads = (0..4)
        .map(|_| {
            let db = db.clone();
            let inputs = inputs.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    for input in &inputs {
                        assert!(*volatile_arc(&db, *input) == LiveValue);
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    for thread in threads {
        thread.join().unwrap();
    }
}

#[cfg(feature = "salsa_unstable")]
#[test]
fn volatile_eviction_is_safe_with_memory_usage() {
    let db = salsa::DatabaseImpl::new();
    let inputs = (0..256)
        .map(|field| MyInput::new(&db, field))
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(2));

    let worker = {
        let db = db.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..10 {
                for input in &inputs {
                    assert!(*volatile_arc(&db, *input) == LiveValue);
                }
            }
        })
    };

    barrier.wait();
    for _ in 0..100 {
        let _ = <dyn salsa::Database>::memory_usage(&db);
    }

    worker.join().unwrap();
}

#[test]
fn volatile_eviction_retains_dependency_info() {
    reset_counts();
    let mut db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);

    assert_eq!(outer_value(&db, input), 23);
    assert_eq!(volatile_executions(), 1);
    assert_eq!(outer_executions(), 1);

    fill_volatile_cache(&db);
    let volatile_after_fill = volatile_executions();
    db.synthetic_write(Durability::HIGH);

    assert_eq!(outer_value(&db, input), 23);
    assert_eq!(volatile_executions(), volatile_after_fill);
    assert_eq!(outer_executions(), 1);
}

#[test]
fn volatile_does_not_evict_cycle_participants() {
    reset_counts();
    let db = salsa::DatabaseImpl::new();
    let input = MyInput::new(&db, 22);

    assert_eq!(volatile_value(&db, input), 22);
    let cycle_value = volatile_cycle_value(&db, input);

    fill_volatile_cache(&db);
    fill_volatile_cycle_cache(&db);

    let volatile_before = volatile_executions();
    let cycle_before = cycle_executions();

    assert_eq!(volatile_value(&db, input), 22);
    assert_eq!(volatile_cycle_value(&db, input), cycle_value);

    assert!(volatile_executions() > volatile_before);
    assert_eq!(cycle_executions(), cycle_before);
}
