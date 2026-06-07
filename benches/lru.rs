use std::hint::black_box;

use codspeed_criterion_compat::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};
use salsa::Database as _;

const HOT_ITEMS: usize = 1_024;
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

fn evenly_spaced_items(items: &[Item], count: usize) -> Vec<Item> {
    assert!(count <= items.len());
    assert_eq!(items.len() % count, 0);

    let step = items.len() / count;
    (0..count).map(|index| items[index * step]).collect()
}

fn lru(criterion: &mut Criterion) {
    let mut group: codspeed_criterion_compat::BenchmarkGroup<
        codspeed_criterion_compat::measurement::WallTime,
    > = criterion.benchmark_group("LRU");

    group.bench_function(BenchmarkId::new("hot_cache_hits", HOT_ITEMS), |b| {
        let db = salsa::DatabaseImpl::new();
        let items = new_items(&db, HOT_ITEMS);
        let expected = prewarm(&db, &items);

        b.iter(|| {
            let actual = access_all(black_box(&db), black_box(&items));
            assert_eq!(black_box(actual), expected);
        });
    });

    group.bench_function(BenchmarkId::new("disabled_capacity_hits", HOT_ITEMS), |b| {
        let mut db = salsa::DatabaseImpl::new();
        lru_value::set_lru_capacity(&mut db, 0);
        let items = new_items(&db, HOT_ITEMS);
        let expected = prewarm(&db, &items);

        b.iter(|| {
            let actual = access_all(black_box(&db), black_box(&items));
            assert_eq!(black_box(actual), expected);
        });
    });

    group.bench_function(
        BenchmarkId::new("working_set_over_capacity", EVICTION_ITEMS),
        |b| {
            b.iter_batched_ref(
                || {
                    let mut db = salsa::DatabaseImpl::new();
                    lru_value::set_lru_capacity(&mut db, EVICTION_CAPACITY);
                    let items = new_items(&db, EVICTION_ITEMS);
                    let expected = prewarm(&db, &items);
                    let hot_items = &items[EVICTION_ITEMS - EVICTION_CAPACITY..];
                    let actual = access_all(&db, hot_items);
                    assert_eq!(black_box(actual), expected_sum(hot_items, &db));
                    db.trigger_lru_eviction();
                    (db, items, expected)
                },
                |(db, items, expected)| {
                    let actual = access_all(black_box(db), black_box(items));
                    assert_eq!(black_box(actual), *expected);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new("scattered_hot_set_over_capacity", EVICTION_ITEMS),
        |b| {
            b.iter_batched_ref(
                || {
                    let mut db = salsa::DatabaseImpl::new();
                    lru_value::set_lru_capacity(&mut db, EVICTION_CAPACITY);
                    let items = new_items(&db, EVICTION_ITEMS);
                    let expected = prewarm(&db, &items);
                    let hot_items = evenly_spaced_items(&items, SCATTERED_HOT_ITEMS);
                    let actual = access_all(&db, &hot_items);
                    assert_eq!(black_box(actual), expected_sum(&hot_items, &db));
                    db.trigger_lru_eviction();
                    (db, items, expected)
                },
                |(db, items, expected)| {
                    let actual = access_all(black_box(db), black_box(items));
                    assert_eq!(black_box(actual), *expected);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.finish();
}

criterion_group!(benches, lru);
criterion_main!(benches);
