//! Compares eviction policies in workloads that require wall-time measurement.
//!
//! The multi-revision workloads perform enough query computation and allocation
//! to make eviction decisions observable, producing instruction traces that are
//! prohibitively expensive to simulate. The concurrent workload requires real
//! scheduling and contention.
//!
//! Every benchmark includes `NoEviction` as the policy-free baseline for how
//! cheap the workload could be without eviction bookkeeping or capacity-driven
//! recomputation.
//!
//! Revision-changing benchmarks use single-iteration samples to avoid batching
//! multiple databases, and 20 samples to keep their setup cost bounded. The
//! concurrent benchmark uses four synchronized workers.
//!
//! Set `SALSA_BENCH_COUNTS=1` to replay `mixed_access` and `phase_change`
//! outside timing and report extra executions relative to NoEviction per
//! revision and seed, with both 80% and 60% popular reads.

use std::hint::black_box;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use salsa::Database as _;

#[path = "eviction/support.rs"]
mod support;

use support::{Benchmark, Item, POLICIES, Policy, Value, access_all, prewarm};

const PARALLEL_WORKERS: usize = 4;

fn main() {
    divan::main();
}

/// Measures a large initial working set followed by small incremental working sets.
///
/// ```text
/// capacity: 256
/// Pn = entry n; repeat(Pn, 64) = 64 consecutive accesses to that entry
///
/// initial revision (1,024 distinct entries):
/// repeat(P0, 64), repeat(P1, 64), ..., repeat(P1023, 64)
///
/// ---- revision boundary ----
///
/// incremental revisions 1-3 (the same 8 entries in every revision):
/// repeat(P0, 64),   repeat(P128, 64), repeat(P256, 64), repeat(P384, 64),
/// repeat(P512, 64), repeat(P640, 64), repeat(P768, 64), repeat(P896, 64)
/// ```
///
/// This resembles checking an entire project once, followed by incremental
/// checks after edits that affect only a few files.
///
/// The initial phase accesses 1,024 entries, far exceeding the cache capacity,
/// and reuses each entry many times before moving to the next one. Later
/// revisions repeatedly access only eight entries. Spreading those entries
/// across the initial access order avoids biasing the result toward the recently
/// accessed tail.
///
/// The initial phase measures admission and eviction while repeated calls hit
/// the current entry. The incremental phases then measure the cost of loading
/// active entries that did not survive and how quickly the policy stabilizes
/// around the smaller working set.
#[divan::bench(args = POLICIES, sample_count = 20, sample_size = 1)]
fn project_check_then_incremental(bencher: divan::Bencher, policy: Policy) {
    policy.bench(bencher, Project);
}

struct Project;

impl Benchmark for Project {
    fn run<I: Item>(self, bencher: divan::Bencher) {
        const CAPACITY: usize = 256;
        const PROJECT_FILES: usize = 1_024;
        const ACTIVE_FILES: usize = 8;
        const CALLS_PER_FILE: usize = 64;
        const INCREMENTAL_REVISIONS: usize = 3;

        bencher
            .with_inputs(|| {
                let (db, files) = I::setup(CAPACITY, PROJECT_FILES);
                let active_files = files
                    .iter()
                    .step_by(PROJECT_FILES / ACTIVE_FILES)
                    .copied()
                    .collect::<Vec<_>>();
                (db, files, active_files)
            })
            .bench_local_refs(|(db, files, active_files)| {
                let mut sum = access_each_repeatedly(&*db, files, CALLS_PER_FILE).0;

                for _ in 0..INCREMENTAL_REVISIONS {
                    db.synthetic_write(salsa::Durability::LOW);
                    sum = sum
                        .wrapping_add(access_each_repeatedly(&*db, active_files, CALLS_PER_FILE).0);
                }

                black_box(Value(sum))
            });
    }
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
#[divan::bench(args = POLICIES, sample_count = 20, sample_size = 1)]
fn scan_resistance(bencher: divan::Bencher, policy: Policy) {
    policy.bench(bencher, Recurring::Scan);
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
#[divan::bench(args = POLICIES, sample_count = 20, sample_size = 1)]
fn one_hit_wonders(bencher: divan::Bencher, policy: Policy) {
    policy.bench(bencher, Recurring::OneHitWonders);
}

enum Recurring {
    Scan,
    OneHitWonders,
}

impl Recurring {
    fn round<I: Copy>(&self, hot: &[I], cold: &[I]) -> Vec<I> {
        let mut reads = Vec::with_capacity(320);
        match self {
            Self::Scan => {
                for (hot, cold) in hot.as_chunks::<3>().0.iter().zip(cold.as_chunks::<2>().0) {
                    reads.extend_from_slice(hot);
                    reads.extend_from_slice(cold);
                }
            }
            Self::OneHitWonders => {
                for (&hot, &cold) in hot.iter().zip(cold) {
                    reads.extend([hot, cold, hot]);
                }
                reads.extend_from_slice(&cold[hot.len()..]);
            }
        }
        reads
    }
}

impl Benchmark for Recurring {
    fn run<I: Item>(self, bencher: divan::Bencher) {
        let (capacity, hot_count, cold_per_round) = match self {
            Self::Scan => (256, 192, 128),
            Self::OneHitWonders => (128, 64, 192),
        };
        bencher
            .with_inputs(|| {
                let (db, items) = I::setup(capacity, hot_count + 5 * cold_per_round);
                let (hot, cold) = items.split_at(hot_count);
                let mut rounds = cold
                    .chunks_exact(cold_per_round)
                    .map(|cold| self.round(hot, cold));
                prewarm(&db, &rounds.next().unwrap());
                // The final probe charges for hot entries displaced by the last round.
                (db, rounds.chain([hot.to_vec()]).collect::<Vec<_>>())
            })
            .bench_local_refs(|(db, rounds)| {
                let mut sum = 0u64;
                for round in rounds {
                    db.synthetic_write(salsa::Durability::LOW);
                    sum = sum.wrapping_add(access_all(db, round.iter().copied()).0);
                }
                black_box(Value(sum))
            });
    }
}

const SEEDS: [u64; 3] = [17, 42, 99];
const REVISIONS: usize = 8;
const KEYS: usize = 1_024;
const POPULAR: usize = 128;
const READS: usize = 2_048;

fn mixed_cases() -> impl Iterator<Item = (Policy, usize)> {
    POLICIES
        .iter()
        .flat_map(|&policy| [(policy, 64), (policy, 256)])
}

/// Each revision draws with replacement from a popular set (80% of reads)
/// and the remaining keys. Only the larger capacity can hold the popular set.
#[divan::bench(args = mixed_cases(), sample_count = 20, sample_size = 1)]
fn mixed_access(bencher: divan::Bencher, (policy, capacity): (Policy, usize)) {
    policy.bench(
        bencher,
        Mixed {
            capacity,
            switch: false,
        },
    );
}

/// Replay the same draws as mixed_access, swapping two sets of 128 keys
/// halfway through. The old popular keys can still receive uncommon reads.
#[divan::bench(args = mixed_cases(), sample_count = 20, sample_size = 1)]
fn phase_change(bencher: divan::Bencher, (policy, capacity): (Policy, usize)) {
    policy.bench(
        bencher,
        Mixed {
            capacity,
            switch: true,
        },
    );
}

struct Mixed {
    capacity: usize,
    switch: bool,
}

impl Mixed {
    fn rounds<I: Item>(&self, items: &[I], seed: u64, popular_percent: u32) -> Vec<Vec<I>> {
        let mut rng = fastrand::Rng::with_seed(seed);
        let mut keys = items.to_vec();
        rng.shuffle(&mut keys);
        let mut rounds = Vec::with_capacity(REVISIONS);
        for revision in 0..REVISIONS {
            if self.switch && revision == REVISIONS / 2 {
                keys[..2 * POPULAR].rotate_left(POPULAR);
            }
            let reads = (0..READS)
                .map(|_| {
                    let range = if rng.u32(..100) < popular_percent {
                        0..POPULAR
                    } else {
                        POPULAR..KEYS
                    };
                    keys[rng.usize(range)]
                })
                .collect();
            rounds.push(reads);
        }
        rounds
    }
}

impl Benchmark for Mixed {
    fn run<I: Item>(self, bencher: divan::Bencher) {
        if std::env::var_os("SALSA_BENCH_COUNTS").is_some() {
            self.report::<I>();
        }

        // Start each trace with a fresh database. Charge each revision for
        // its sweep, including the last. Sequence generation is untimed.
        bencher
            .with_inputs(|| {
                SEEDS.map(|seed| {
                    let (db, items) = I::setup(self.capacity, KEYS);
                    let rounds = self.rounds(&items, seed, 80);
                    (db, rounds)
                })
            })
            .bench_local_refs(|traces| {
                for (db, rounds) in traces {
                    for round in rounds {
                        mixed_revision(db, round);
                    }
                }
            });
    }
}

fn mixed_revision<I: Item>(db: &mut dyn salsa::Database, round: &[I]) {
    black_box(access_all(db, round.iter().copied()));
    db.synthetic_write(salsa::Durability::LOW);
}

// Only the untimed replay installs an event callback.
#[salsa::db]
#[derive(Clone)]
struct CountingDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for CountingDb {}

impl Mixed {
    fn executions<I: Item>(&self, seed: u64, popular_percent: u32) -> [usize; REVISIONS] {
        let executions = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = executions.clone();
        let mut db = CountingDb {
            storage: salsa::Storage::new(Some(Box::new(move |event| {
                if matches!(event.kind, salsa::EventKind::WillExecute { .. }) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }))),
        };
        let items = I::items(&mut db, self.capacity, KEYS);
        let rounds = self.rounds(&items, seed, popular_percent);
        std::array::from_fn(|revision| {
            mixed_revision(&mut db, &rounds[revision]);
            executions.swap(0, Ordering::Relaxed)
        })
    }

    fn report<I: Item>(&self) {
        let workload = if self.switch {
            "phase_change"
        } else {
            "mixed_access"
        };
        let policy = std::any::type_name::<I>().rsplit("::").next().unwrap();
        for popular_percent in [80, 60] {
            for seed in SEEDS {
                let baseline = self.executions::<support::NoEviction>(seed, popular_percent);
                let actual = self.executions::<I>(seed, popular_percent);
                let extra: [_; REVISIONS] = std::array::from_fn(|i| actual[i] - baseline[i]);
                eprintln!(
                    "{workload} {policy} capacity={} popular={popular_percent}% seed={seed} extra={extra:?}",
                    self.capacity
                );
            }
        }
    }
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
/// Divan synchronizes the workers before each timed sample, avoiding variance
/// from Rayon task dispatch and workers joining the workload at different times.
#[divan::bench(args = POLICIES, threads = PARALLEL_WORKERS, sample_count = 20, sample_size = 1)]
fn parallel_fast_path(bencher: divan::Bencher, policy: Policy) {
    policy.bench(bencher, Parallel);
}

struct Parallel;

impl Benchmark for Parallel {
    fn run<I: Item>(self, bencher: divan::Bencher) {
        const ACCESSES_PER_WORKER: usize = 4_096;

        let (db, items) = I::setup(PARALLEL_WORKERS, PARALLEL_WORKERS);
        prewarm(&db, &items);

        let db = Mutex::new(db);
        let next_worker = AtomicUsize::new(0);

        bencher
            .with_inputs(|| {
                let worker = next_worker.fetch_add(1, Ordering::Relaxed);
                (db.lock().unwrap().clone(), items[worker % items.len()])
            })
            .bench_refs(|(db, item)| {
                black_box(access_all(
                    db,
                    std::iter::repeat_n(*item, ACCESSES_PER_WORKER),
                ))
            });
    }
}

fn access_each_repeatedly<I: Item>(db: &dyn salsa::Database, items: &[I], count: usize) -> Value {
    access_all(
        db,
        items
            .iter()
            .copied()
            .flat_map(|item| std::iter::repeat_n(item, count)),
    )
}
