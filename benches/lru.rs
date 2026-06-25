use std::hint::black_box;

use rayon::prelude::*;
use salsa::Database as _;

fn main() {
    divan::main();
}

const HOT_ITEMS: usize = 1_024;
const CONCURRENT_WORKERS: usize = 4;
const EVICTION_ITEMS: usize = 4_096;
const EVICTION_CAPACITY: usize = 512;
const SCATTERED_HOT_ITEMS: usize = EVICTION_CAPACITY * 2;

#[salsa::input]
struct Item {
    value: usize,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
struct Value(usize);

impl Drop for Value {
    fn drop(&mut self) {
        black_box(self);
    }
}

#[salsa::tracked(lru = 4096)]
#[inline(never)]
fn lru_value(db: &dyn salsa::Database, item: Item) -> Value {
    value(item.value(db))
}

#[inline(never)]
fn value(value: usize) -> Value {
    Value(value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223))
}

fn new_items(db: &salsa::DatabaseImpl, count: usize) -> Vec<Item> {
    (0..count)
        .map(|value| Item::new(black_box(db), black_box(value)))
        .collect()
}

#[inline(never)]
fn access_all(db: &dyn salsa::Database, items: &[Item]) -> Value {
    let mut sum = 0usize;
    for item in items {
        sum = sum.wrapping_add(lru_value(black_box(db), black_box(*item)).0);
    }
    Value(sum)
}

fn expected_sum(items: &[Item], db: &dyn salsa::Database) -> Value {
    let mut sum = 0usize;
    for item in items {
        sum = sum.wrapping_add(value(item.value(db)).0);
    }
    Value(sum)
}

fn prewarm(db: &dyn salsa::Database, items: &[Item]) -> Value {
    let actual = access_all(db, items);
    let expected = expected_sum(items, db);
    assert_eq!(black_box(actual.clone()), expected);
    actual
}

#[inline(never)]
fn access_all_and_collect(db: &mut salsa::DatabaseImpl, items: &[Item]) -> Value {
    let actual = access_all(black_box(&*db), black_box(items));
    db.trigger_lru_eviction();
    actual
}

#[inline(never)]
fn access_all_concurrently(
    pool: &rayon::ThreadPool,
    db: &salsa::DatabaseImpl,
    items: &[Item],
) -> Vec<Value> {
    let worker_dbs: Vec<_> = (0..CONCURRENT_WORKERS).map(|_| db.clone()).collect();
    pool.install(|| {
        worker_dbs
            .into_par_iter()
            .map(|db| access_all(black_box(&db), black_box(items)))
            .collect()
    })
}

fn hot_cache() -> (salsa::DatabaseImpl, Vec<Item>, Value) {
    let db = salsa::DatabaseImpl::new();
    let items = new_items(&db, HOT_ITEMS);
    let expected = prewarm(&db, &items);
    (db, items, expected)
}

fn hot_cache_for_sweep() -> (salsa::DatabaseImpl, Vec<Item>, Value) {
    let mut db = salsa::DatabaseImpl::new();
    lru_value::set_lru_capacity(&mut db, HOT_ITEMS);
    let items = new_items(&db, HOT_ITEMS);
    let expected = prewarm(&db, &items);
    (db, items, expected)
}

fn evenly_spaced_items(items: &[Item], count: usize) -> Vec<Item> {
    assert!(count <= items.len());
    assert_eq!(items.len() % count, 0);

    let step = items.len() / count;
    (0..count).map(|index| items[index * step]).collect()
}

#[divan::bench(name = "hot_cache_hits[1024]")]
fn hot_cache_hits(bencher: divan::Bencher) {
    let (db, items, expected) = hot_cache();

    bencher.bench_local(|| {
        let actual = access_all(black_box(&db), black_box(&items));
        assert_eq!(black_box(actual), expected);
    });
}

#[divan::bench(name = "concurrent_hot_cache_hits[4x1024]")]
fn concurrent_hot_cache_hits(bencher: divan::Bencher) {
    let (db, items, expected) = hot_cache();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(CONCURRENT_WORKERS)
        .build()
        .unwrap();

    bencher.bench_local(|| {
        for actual in access_all_concurrently(&pool, black_box(&db), black_box(&items)) {
            assert_eq!(black_box(actual), expected);
        }
    });
}

#[divan::bench(
    name = "hot_cache_hits_and_sweep[1024]",
    sample_count = 20,
    sample_size = 1
)]
fn hot_cache_hits_and_sweep(bencher: divan::Bencher) {
    bencher
        .with_inputs(hot_cache_for_sweep)
        .bench_local_refs(|(db, items, expected)| {
            let actual = access_all_and_collect(black_box(db), black_box(items));
            assert_eq!(black_box(actual), *expected);
        });
}

#[divan::bench(
    name = "concurrent_hot_cache_hits_and_sweep[4x1024]",
    sample_count = 20,
    sample_size = 1
)]
fn concurrent_hot_cache_hits_and_sweep(bencher: divan::Bencher) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(CONCURRENT_WORKERS)
        .build()
        .unwrap();

    bencher
        .with_inputs(hot_cache_for_sweep)
        .bench_local_refs(|(db, items, expected)| {
            for actual in access_all_concurrently(&pool, black_box(db), black_box(items)) {
                assert_eq!(black_box(actual), *expected);
            }
            db.trigger_lru_eviction();
        });
}

#[divan::bench(name = "disabled_capacity_hits[1024]")]
fn disabled_capacity_hits(bencher: divan::Bencher) {
    let mut db = salsa::DatabaseImpl::new();
    lru_value::set_lru_capacity(&mut db, 0);
    let items = new_items(&db, HOT_ITEMS);
    let expected = prewarm(&db, &items);

    bencher.bench_local(|| {
        let actual = access_all(black_box(&db), black_box(&items));
        assert_eq!(black_box(actual), expected);
    });
}

#[divan::bench(
    name = "working_set_over_capacity[4096]",
    sample_count = 20,
    sample_size = 1
)]
fn working_set_over_capacity(bencher: divan::Bencher) {
    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            lru_value::set_lru_capacity(&mut db, EVICTION_CAPACITY);
            let items = new_items(&db, EVICTION_ITEMS);
            let expected = prewarm(&db, &items);
            let hot_items = &items[EVICTION_ITEMS - EVICTION_CAPACITY..];
            let actual = access_all(&db, hot_items);
            assert_eq!(black_box(actual), expected_sum(hot_items, &db));
            db.trigger_lru_eviction();
            (db, items, expected)
        })
        .bench_local_refs(|(db, items, expected)| {
            let actual = access_all_and_collect(black_box(db), black_box(items));
            assert_eq!(black_box(actual), *expected);
        });
}

#[divan::bench(
    name = "scattered_hot_set_over_capacity[4096]",
    sample_count = 20,
    sample_size = 1
)]
fn scattered_hot_set_over_capacity(bencher: divan::Bencher) {
    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            lru_value::set_lru_capacity(&mut db, EVICTION_CAPACITY);
            let items = new_items(&db, EVICTION_ITEMS);
            let expected = prewarm(&db, &items);
            let hot_items = evenly_spaced_items(&items, SCATTERED_HOT_ITEMS);
            let actual = access_all(&db, &hot_items);
            assert_eq!(black_box(actual), expected_sum(&hot_items, &db));
            db.trigger_lru_eviction();
            (db, items, expected)
        })
        .bench_local_refs(|(db, items, expected)| {
            let actual = access_all_and_collect(black_box(db), black_box(items));
            assert_eq!(black_box(actual), *expected);
        });
}
