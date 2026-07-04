//! Measures eviction-policy synchronization overhead under concurrent access.
//!
//! This benchmark runs separately from the single-threaded eviction workloads
//! because real scheduling and contention require wall-time measurement.

use std::hint::black_box;

use rayon::prelude::*;

#[path = "eviction/support.rs"]
mod support;

use support::{
    Item, Policy, Value, access_all_with, lru_value, new_items, no_eviction_value, prewarm,
};

fn main() {
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build_global()
        .unwrap();

    divan::main();
}

/// Measures synchronization overhead on concurrent cache hits.
///
/// ```text
/// worker 0: A A A ... (4,096 accesses)
/// worker 1: B B B ...
/// worker 2: C C C ...
/// worker 3: D D D ...
/// ```
///
/// Each worker repeatedly accesses a different prewarmed query, avoiding both
/// misses and contention on the query itself. This isolates contention in the
/// eviction policy's hit bookkeeping; `NoEviction` provides the baseline.
#[divan::bench(
    args = [Policy::NoEviction, Policy::Lru],
    sample_count = 20,
    sample_size = 1
)]
fn parallel_fast_path(bencher: divan::Bencher, policy: Policy) {
    const ACCESSES_PER_WORKER: usize = 4_096;

    bencher
        .with_inputs(|| {
            let worker_count = rayon::current_num_threads();
            let mut db = salsa::DatabaseImpl::new();
            policy.set_capacity(&mut db, worker_count);
            let items = new_items(&db, worker_count);
            prewarm(policy, &db, &items);

            items
                .into_iter()
                .map(|item| (db.clone(), item))
                .collect::<Vec<_>>()
        })
        .bench_local_refs(|jobs| {
            black_box(parallel_access_repeated(policy, jobs, ACCESSES_PER_WORKER))
        });
}

fn parallel_access_repeated(
    policy: Policy,
    jobs: &mut [(salsa::DatabaseImpl, Item)],
    accesses_per_item: usize,
) -> Value {
    match policy {
        Policy::NoEviction => {
            parallel_access_repeated_with(jobs, accesses_per_item, no_eviction_value)
        }
        Policy::Lru => parallel_access_repeated_with(jobs, accesses_per_item, lru_value),
    }
}

fn parallel_access_repeated_with(
    jobs: &mut [(salsa::DatabaseImpl, Item)],
    accesses_per_item: usize,
    fetch: impl Fn(&dyn salsa::Database, Item) -> Value + Copy + Send + Sync,
) -> Value {
    let sum = jobs
        .par_iter_mut()
        .map(|(db, item)| {
            access_all_with(db, std::iter::repeat_n(*item, accesses_per_item), fetch).0
        })
        .reduce(|| 0, u64::wrapping_add);
    Value(sum)
}
