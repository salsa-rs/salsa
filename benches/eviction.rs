//! Compares eviction policies across cache-hit overhead, eviction cost, and
//! cache efficiency.
//!
//! Each benchmark isolates one access pattern. Multi-revision workloads expose
//! poor eviction decisions through later recomputation.
//! Concurrent hit-path contention is measured separately by
//! `eviction_parallel` so these workloads can use CPU and memory instrumentation.
//!
//! Every benchmark includes `NoEviction` as the policy-free baseline for how
//! cheap the workload could be without eviction bookkeeping or capacity-driven
//! recomputation.
//!
//! Database-owning benchmarks use single-iteration samples to avoid batching
//! multiple databases, and 20 samples to keep their setup cost bounded.

use std::hint::black_box;

use salsa::Database as _;

#[path = "eviction/support.rs"]
mod support;

use support::{
    Item, Policy, Value, access_all, access_all_with, lru_value, new_items, no_eviction_value,
    prewarm,
};

fn main() {
    divan::main();
}

/// Measures the overhead each eviction policy adds to repeated cache hits.
///
/// ```text
/// prewarm: A B C ... (1,024 entries)
/// timed:   A B C ... | A B C ... | ...
/// ```
///
/// Hot query results are often reused many times within one revision, so even
/// small per-hit costs can add up. Every result remains resident, which isolates
/// hit-path bookkeeping. `NoEviction` shows the tracked-query cost without an
/// eviction policy.
#[divan::bench(args = [Policy::NoEviction, Policy::Lru])]
fn fast_path(bencher: divan::Bencher, policy: Policy) {
    const ITEMS: usize = 1_024;

    let mut db = salsa::DatabaseImpl::new();
    policy.set_capacity(&mut db, ITEMS);
    let items = new_items(&db, ITEMS);
    prewarm(policy, &db, &items);

    bencher.bench_local(|| black_box(access_all(policy, &db, items.iter().copied())));
}

/// Measures cache-hit overhead after disabling eviction at runtime.
///
/// ```text
/// capacity: 0
/// prewarm:  A B C ... (1,024 entries)
/// timed:    A B C ... | A B C ... | ...
/// ```
///
/// Setting capacity to zero disables an eviction policy without changing the
/// tracked function. This measures the remaining disabled-policy branch on the
/// hit path. `NoEviction` is the policy-free reference for how cheap this path
/// could be.
#[divan::bench(args = [Policy::NoEviction, Policy::Lru])]
fn disabled_eviction(bencher: divan::Bencher, policy: Policy) {
    const ITEMS: usize = 1_024;

    let mut db = salsa::DatabaseImpl::new();
    policy.set_capacity(&mut db, 0);
    let items = new_items(&db, ITEMS);
    prewarm(policy, &db, &items);

    bencher.bench_local(|| black_box(access_all(policy, &db, items.iter().copied())));
}

/// Measures the cost of enforcing capacity when the working set already fits.
///
/// ```text
/// prewarm: A B C ... (1,024 entries)
/// timed:   A B C ... -> enforce capacity 1,024 -> no victim
/// ```
///
/// A capacity check should be cheap when no victim is needed. The benchmark
/// reads a fully resident working set and then triggers a sweep, separating
/// steady-state sweep bookkeeping from victim selection and destruction.
#[divan::bench(
    args = [Policy::NoEviction, Policy::Lru],
    sample_count = 20,
    sample_size = 1
)]
fn fast_path_and_sweep(bencher: divan::Bencher, policy: Policy) {
    const ITEMS: usize = 1_024;

    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            policy.set_capacity(&mut db, ITEMS);
            let items = new_items(&db, ITEMS);
            prewarm(policy, &db, &items);
            (db, items)
        })
        .bench_local_refs(|(db, items)| {
            let result = access_all(policy, &*db, items.iter().copied());
            db.trigger_lru_eviction();
            black_box(result)
        });
}

/// Measures the cost of exceeding capacity and evicting the excess entries.
///
/// ```text
/// timed: empty -> A B C ... (320 entries) -> enforce capacity 256
///                                             `-> evict 64
/// ```
///
/// The cache starts empty, admits 320 entries into a capacity of 256, and then
/// enforces that capacity. This includes admission, eager or lazy victim
/// selection, and value destruction. `NoEviction` provides the computation
/// baseline; setup and final database destruction remain outside timing.
#[divan::bench(
    args = [Policy::NoEviction, Policy::Lru],
    sample_count = 20,
    sample_size = 1
)]
fn fill_and_evict(bencher: divan::Bencher, policy: Policy) {
    const CAPACITY: usize = 256;
    const ITEMS: usize = 320;

    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            policy.set_capacity(&mut db, CAPACITY);
            let items = new_items(&db, ITEMS);
            (db, items)
        })
        .bench_local_refs(|(db, items)| {
            let result = access_all(policy, &*db, items.iter().copied());
            db.trigger_lru_eviction();
            black_box(result)
        });
}

/// Measures resistance to cache pollution from an interleaved one-shot scan.
///
/// ```text
/// H = next recurring entry; Sr = next scan entry from round r
///
/// warmup:     [H H H S0 S0] x 64
///                         |
///                      revision
///                         v
/// rounds 1..4: [H H H Sr Sr] x 64  (revision after each round)
///                         |
///                         v
/// probe:       [H] x 192
/// ```
///
/// Each mixed round accesses the same 192 recurring entries exactly once and
/// interleaves 128 entries that are never used again. The recurring set leaves
/// 64 free slots in a cache of 256, so each 128-entry scan must displace cached
/// entries. Every round uses the same access order and provides no same-revision
/// frequency signal. After an untimed mixed round establishes cache state, four
/// measured rounds test which recurring entries survive the scans. A final
/// recurring-only round charges for entries displaced by the last scan.
/// `NoEviction` shows the cost when recurring values are never displaced.
#[divan::bench(
    args = [Policy::NoEviction, Policy::Lru],
    sample_count = 20,
    sample_size = 1
)]
fn scan_resistance(bencher: divan::Bencher, policy: Policy) {
    const CAPACITY: usize = 256;
    const HOT_ITEMS: usize = 192;
    const ITEMS_PER_ROUND: usize = 128;
    const ROUNDS: usize = 5;

    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            policy.set_capacity(&mut db, CAPACITY);
            let items = new_items(&db, HOT_ITEMS + ITEMS_PER_ROUND * ROUNDS);
            let hot = &items[..HOT_ITEMS];

            let mut rounds = Vec::with_capacity(ROUNDS);

            for round in 0..ROUNDS {
                let cold_start = HOT_ITEMS + round * ITEMS_PER_ROUND;
                let cold_end = cold_start + ITEMS_PER_ROUND;
                let cold = &items[cold_start..cold_end];
                let mut accesses = Vec::with_capacity(HOT_ITEMS + ITEMS_PER_ROUND);

                // Mix three reusable entries with every two one-shot entries.
                for (hot_chunk, cold_chunk) in hot.chunks_exact(3).zip(cold.chunks_exact(2)) {
                    accesses.extend_from_slice(hot_chunk);
                    accesses.extend_from_slice(cold_chunk);
                }
                rounds.push(accesses);
            }

            prewarm(policy, &db, &rounds.remove(0));

            // Pay for recurring entries displaced by the final mixed round.
            rounds.push(hot.to_vec());

            Workload { db, rounds }
        })
        .bench_local_refs(|workload| black_box(workload.run(policy)));
}

/// Measures whether a policy retains popular queries over one-hit wonders.
///
/// ```text
/// round 0 (untimed): H0 C0 H0 | H1 C1 H1 | ... | H63 C63 H63 | C64 ... C191
///                                      |
///                                   revision
///                                      v
/// round 1:           H0 C192 H0 | H1 C193 H1 | ... | H63 C255 H63 | C256 ... C383
///                                      |
///                                   revision
///                                      v
/// round 2:           H0 C384 H0 | H1 C385 H1 | ... | H63 C447 H63 | C448 ... C575
///                                      |
///                                     ...
///                                      |
///                                   revision
///                                      v
/// final probe:       H0 H1 ... H63
/// ```
///
/// Each round accesses 64 popular entries twice, with a one-hit wonder between
/// each pair, then admits 128 more entries that are never used again. That final
/// cold tail fills the capacity, so LRU sees every one-hit wonder as more recent
/// than the popular entries and evicts the values needed by the next round.
/// A frequency-aware policy can use the second popular access to distinguish
/// reusable entries from one-hit wonders.
///
/// Round 0 establishes policy state outside the timed region. Four measured
/// rounds charge for previous eviction decisions, and the final probe charges
/// for popular entries displaced by the last one-hit-wonder batch. `NoEviction`
/// provides the computation baseline; retaining cold entries cannot create hits
/// because they are never accessed again.
#[divan::bench(
    args = [Policy::NoEviction, Policy::Lru],
    sample_count = 20,
    sample_size = 1
)]
fn one_hit_wonders(bencher: divan::Bencher, policy: Policy) {
    const CAPACITY: usize = 128;
    const HOT_ITEMS: usize = 64;
    const COLD_ITEMS_PER_ROUND: usize = 192;
    const ROUNDS: usize = 5;

    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            policy.set_capacity(&mut db, CAPACITY);
            let items = new_items(&db, HOT_ITEMS + COLD_ITEMS_PER_ROUND * ROUNDS);
            let hot = &items[..HOT_ITEMS];
            let mut rounds = Vec::with_capacity(ROUNDS);

            for round in 0..ROUNDS {
                let cold_start = HOT_ITEMS + round * COLD_ITEMS_PER_ROUND;
                let cold_end = cold_start + COLD_ITEMS_PER_ROUND;
                let cold = &items[cold_start..cold_end];
                let mut accesses = Vec::with_capacity(HOT_ITEMS * 2 + COLD_ITEMS_PER_ROUND);

                for (hot_item, cold_item) in hot.iter().zip(cold) {
                    accesses.push(*hot_item);
                    accesses.push(*cold_item);
                    accesses.push(*hot_item);
                }
                accesses.extend_from_slice(&cold[HOT_ITEMS..]);
                rounds.push(accesses);
            }

            prewarm(policy, &db, &rounds.remove(0));

            rounds.push(hot.to_vec());

            Workload { db, rounds }
        })
        .bench_local_refs(|workload| black_box(workload.run(policy)));
}

/// Measures how quickly a policy abandons a working set that is no longer used.
///
/// ```text
/// [A x 192] --revision--> [B x 192] --revision--> [B x 192]
///                                      --revision--> [B x 192]
/// ```
///
/// The benchmark switches from 192 entries to a disjoint set of
/// 192 entries. Either set fits by itself, so retaining the old phase only
/// delays admission of useful results from the new phase. `NoEviction` provides
/// the policy-free baseline: it retains both phases, but only the second phase
/// is accessed after the switch.
#[divan::bench(
    args = [Policy::NoEviction, Policy::Lru],
    sample_count = 20,
    sample_size = 1
)]
fn phase_change(bencher: divan::Bencher, policy: Policy) {
    const CAPACITY: usize = 256;
    const ITEMS_PER_PHASE: usize = 192;
    const ROUNDS: usize = 3;

    bencher
        .with_inputs(|| {
            let mut db = salsa::DatabaseImpl::new();
            policy.set_capacity(&mut db, CAPACITY);
            let items = new_items(&db, ITEMS_PER_PHASE * 2);
            let first_phase = &items[..ITEMS_PER_PHASE];
            let second_phase = &items[ITEMS_PER_PHASE..];
            prewarm(policy, &db, first_phase);

            Workload {
                db,
                rounds: vec![second_phase.to_vec(); ROUNDS],
            }
        })
        .bench_local_refs(|workload| black_box(workload.run(policy)));
}

struct Workload {
    db: salsa::DatabaseImpl,
    /// Each round starts a new revision, applying evictions selected by the
    /// preceding round before performing the next batch of accesses.
    rounds: Vec<Vec<Item>>,
}

impl Workload {
    #[inline]
    fn run(&mut self, policy: Policy) -> Value {
        match policy {
            Policy::NoEviction => self.run_with(no_eviction_value),
            Policy::Lru => self.run_with(lru_value),
        }
    }

    #[inline]
    fn run_with(&mut self, fetch: impl Fn(&dyn salsa::Database, Item) -> Value + Copy) -> Value {
        let mut sum = 0u64;
        for round in &self.rounds {
            self.db.synthetic_write(salsa::Durability::LOW);
            sum = sum.wrapping_add(access_all_with(&self.db, round.iter().copied(), fetch).0);
        }
        Value(sum)
    }
}
