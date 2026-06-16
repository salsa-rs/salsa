use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use codspeed_criterion_compat::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};
use rayon::prelude::*;
use salsa::Database as _;
use salsa::Setter as _;

const HOT_ITEMS: usize = 1_024;
const CONCURRENT_WORKERS: usize = 4;

const CHURN_CAPACITY: usize = 1_024;
const CHURN_HOT_ITEMS: usize = 256;
const COLD_ITEMS_PER_REVISION: usize = 512;
const RECLAMATION_REVISIONS: usize = 16;
const RECLAMATION_DRAIN_REVISIONS: usize = 4;
const MEASURED_REVISIONS: usize = RECLAMATION_REVISIONS;
const DEAD_ITEMS: usize = 4_096;
const QUIESCENT_REVISIONS: usize = RECLAMATION_REVISIONS + RECLAMATION_DRAIN_REVISIONS;
const LARGE_BURST_ITEMS: usize = 65_536;
const BURST_DECAY_REVISIONS: usize = 64;
const ADMISSION_RATES: [usize; 3] = [256, 1_024, 4_096];
const ADMISSION_RATE_REVISIONS: usize = 32;
const SPARSE_TABLE_ROWS: [usize; 2] = [10_000, 100_000];
const SPARSE_RESIDENTS: usize = 1_024;
const SPARSE_REVISIONS: usize = RECLAMATION_REVISIONS;
const PHASE_ITEMS: usize = 1_024;
const PHASE_WARM_REVISIONS: usize = RECLAMATION_REVISIONS;
const PHASE_MEASURED_REVISIONS: usize = 32;
const STEADY_COLLECTION_WARMUPS: usize = RECLAMATION_REVISIONS;
const SCAN_ITEMS: usize = 4_096;
const SCAN_HOT_WARM_REVISIONS: usize = RECLAMATION_REVISIONS;
const SCAN_SETTLE_REVISIONS: usize = RECLAMATION_REVISIONS + RECLAMATION_DRAIN_REVISIONS;
const CYCLIC_ITEMS: usize = 8_192;
const CYCLIC_REVISIONS: usize = 32;
const SKEWED_ITEMS: usize = 8_192;
const SKEWED_WARM_ITEMS: usize = 1_024;
const SKEWED_ACCESSES_PER_REVISION: usize = 4_096;
const SKEWED_REVISIONS: usize = 32;
const WARMUP_RECLAIM_TARGET: usize = COLD_ITEMS_PER_REVISION;
const MAX_WARMUP_REVISIONS: usize = 32;
const MIN_MEASURED_RECLAIM: usize = COLD_ITEMS_PER_REVISION;
const COLD_CAPACITY: usize = CHURN_CAPACITY - CHURN_HOT_ITEMS;
const MIN_QUIESCENT_RECLAIM: usize = DEAD_ITEMS - COLD_CAPACITY;
const MIN_SCAN_RECLAIM: usize = SCAN_ITEMS - COLD_CAPACITY;
const MAX_HOT_RECOMPUTATIONS: usize = MEASURED_REVISIONS * CHURN_HOT_ITEMS / 100;
const MAX_QUIESCENT_HOT_RECOMPUTATIONS: usize = QUIESCENT_REVISIONS * CHURN_HOT_ITEMS / 50;
const MAX_CYCLIC_HOT_RECOMPUTATIONS: usize = CYCLIC_REVISIONS * CHURN_HOT_ITEMS / 100;

#[derive(Debug, Default)]
struct TrackerData {
    live: AtomicUsize,
    dropped: AtomicUsize,
    executions: AtomicUsize,
}

#[derive(Debug, Clone, Default)]
struct Tracker(Arc<TrackerData>);

impl Tracker {
    fn live(&self) -> usize {
        self.0.live.load(Ordering::Relaxed)
    }

    fn dropped(&self) -> usize {
        self.0.dropped.load(Ordering::Relaxed)
    }

    fn executions(&self) -> usize {
        self.0.executions.load(Ordering::Relaxed)
    }
}

impl PartialEq for Tracker {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Tracker {}

#[salsa::input]
struct Item {
    value: usize,
    tracker: Tracker,
}

#[salsa::input]
struct Clock {
    revision: usize,
}

#[derive(PartialEq, Eq, Debug)]
struct Payload {
    value: usize,
    tracker: Tracker,
}

impl Payload {
    fn new(value: usize, tracker: Tracker) -> Self {
        tracker.0.live.fetch_add(1, Ordering::Relaxed);
        Self { value, tracker }
    }
}

impl Drop for Payload {
    fn drop(&mut self) {
        self.tracker.0.live.fetch_sub(1, Ordering::Relaxed);
        self.tracker.0.dropped.fetch_add(1, Ordering::Relaxed);
        black_box(self);
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
struct Value(Arc<Payload>);

impl Value {
    fn new(value: usize, tracker: Tracker) -> Self {
        Self(Arc::new(Payload::new(value, tracker)))
    }

    fn get(&self) -> usize {
        self.0.value
    }
}

#[salsa::tracked(lru = 4096)]
#[inline(never)]
fn lru_value(db: &dyn salsa::Database, item: Item) -> Value {
    let tracker = item.tracker(db);
    tracker.0.executions.fetch_add(1, Ordering::Relaxed);
    Value::new(compute_value(item.value(db)), tracker)
}

#[salsa::tracked(lru = 4096)]
#[inline(never)]
fn plain_lru_value(db: &dyn salsa::Database, item: Item) -> usize {
    compute_value(item.value(db))
}

#[inline(never)]
fn compute_value(value: usize) -> usize {
    value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

fn new_items(db: &salsa::DatabaseImpl, tracker: &Tracker, start: usize, count: usize) -> Vec<Item> {
    (start..start + count)
        .map(|value| Item::new(black_box(db), black_box(value), tracker.clone()))
        .collect()
}

#[inline(never)]
fn access_all(db: &dyn salsa::Database, items: &[Item]) -> usize {
    let mut sum = 0usize;
    for item in items {
        sum = sum.wrapping_add(lru_value(black_box(db), black_box(*item)).get());
    }
    sum
}

#[inline(never)]
fn access_all_plain(db: &dyn salsa::Database, items: &[Item]) -> usize {
    let mut sum = 0usize;
    for item in items {
        sum = sum.wrapping_add(plain_lru_value(black_box(db), black_box(*item)));
    }
    sum
}

fn expected_sum(items: &[Item], db: &dyn salsa::Database) -> usize {
    let mut sum = 0usize;
    for item in items {
        sum = sum.wrapping_add(compute_value(item.value(db)));
    }
    sum
}

fn access_and_assert(db: &dyn salsa::Database, items: &[Item], expected: usize) {
    assert_eq!(black_box(access_all(db, items)), expected);
}

fn access_plain_and_assert(db: &dyn salsa::Database, items: &[Item], expected: usize) {
    assert_eq!(black_box(access_all_plain(db, items)), expected);
}

#[inline(never)]
fn access_all_concurrently(
    pool: &rayon::ThreadPool,
    db: &salsa::DatabaseImpl,
    items: &[Item],
) -> Vec<usize> {
    let worker_dbs: Vec<_> = (0..CONCURRENT_WORKERS).map(|_| db.clone()).collect();
    pool.install(|| {
        worker_dbs
            .into_par_iter()
            .map(|db| access_all(black_box(&db), black_box(items)))
            .collect()
    })
}

#[inline(never)]
fn access_all_plain_concurrently(
    pool: &rayon::ThreadPool,
    db: &salsa::DatabaseImpl,
    items: &[Item],
) -> Vec<usize> {
    let worker_dbs: Vec<_> = (0..CONCURRENT_WORKERS).map(|_| db.clone()).collect();
    pool.install(|| {
        worker_dbs
            .into_par_iter()
            .map(|db| access_all_plain(black_box(&db), black_box(items)))
            .collect()
    })
}

#[inline(never)]
fn access_partitioned_concurrently(
    pool: &rayon::ThreadPool,
    db: &salsa::DatabaseImpl,
    items: &[Item],
) -> usize {
    let chunk_size = items.len().div_ceil(CONCURRENT_WORKERS);
    let worker_dbs: Vec<_> = (0..CONCURRENT_WORKERS).map(|_| db.clone()).collect();
    let partials: Vec<_> = pool.install(|| {
        worker_dbs
            .into_par_iter()
            .zip(items.par_chunks(chunk_size))
            .map(|(db, items)| access_all(black_box(&db), black_box(items)))
            .collect()
    });
    partials
        .into_iter()
        .fold(0usize, |sum, partial| sum.wrapping_add(partial))
}

fn thread_pool() -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(CONCURRENT_WORKERS)
        .build()
        .unwrap()
}

fn hot_cache() -> (salsa::DatabaseImpl, Vec<Item>, usize) {
    let db = salsa::DatabaseImpl::new();
    let tracker = Tracker::default();
    let items = new_items(&db, &tracker, 0, HOT_ITEMS);
    let expected = expected_sum(&items, &db);
    access_plain_and_assert(&db, &items, expected);
    (db, items, expected)
}

fn hot_cache_for_collection() -> (salsa::DatabaseImpl, Vec<Item>, usize, Tracker, usize) {
    let mut db = salsa::DatabaseImpl::new();
    lru_value::set_lru_capacity(&mut db, HOT_ITEMS);
    let tracker = Tracker::default();
    let items = new_items(&db, &tracker, 0, HOT_ITEMS);
    let expected = expected_sum(&items, &db);
    access_and_assert(&db, &items, expected);
    let executions = tracker.executions();
    (db, items, expected, tracker, executions)
}

fn hot_cache_for_steady_collection() -> (
    salsa::DatabaseImpl,
    Clock,
    usize,
    Vec<Item>,
    usize,
    Tracker,
    usize,
) {
    let (mut db, items, expected, tracker, _) = hot_cache_for_collection();
    let clock = Clock::new(&db, 0);
    for revision in 1..=STEADY_COLLECTION_WARMUPS {
        clock.set_revision(&mut db).to(revision);
        access_and_assert(&db, &items, expected);
    }
    let executions = tracker.executions();
    (
        db,
        clock,
        STEADY_COLLECTION_WARMUPS + 1,
        items,
        expected,
        tracker,
        executions,
    )
}

fn trigger_collection_without_reclaiming(db: &mut salsa::DatabaseImpl, tracker: &Tracker) {
    let live = tracker.live();
    let dropped = tracker.dropped();
    db.trigger_lru_eviction();
    assert_eq!(tracker.live(), live);
    assert_eq!(tracker.dropped(), dropped);
}

struct ChurnFixture {
    db: salsa::DatabaseImpl,
    clock: Clock,
    tracker: Tracker,
    hot: Vec<Item>,
    hot_expected: usize,
    next_value: usize,
    revision: usize,
    hot_recomputations: usize,
    measured_dropped: usize,
    measured_executions: usize,
}

impl ChurnFixture {
    fn new() -> Self {
        let mut db = salsa::DatabaseImpl::new();
        lru_value::set_lru_capacity(&mut db, CHURN_CAPACITY);
        let clock = Clock::new(&db, 0);
        let tracker = Tracker::default();
        let hot = new_items(&db, &tracker, 0, CHURN_HOT_ITEMS);
        let hot_expected = expected_sum(&hot, &db);
        access_and_assert(&db, &hot, hot_expected);
        let measured_dropped = tracker.dropped();
        let measured_executions = tracker.executions();

        Self {
            db,
            clock,
            tracker,
            hot,
            hot_expected,
            next_value: CHURN_HOT_ITEMS,
            revision: 0,
            hot_recomputations: 0,
            measured_dropped,
            measured_executions,
        }
    }

    fn advance_revision(&mut self) {
        self.revision += 1;
        self.clock.set_revision(&mut self.db).to(self.revision);
    }

    fn run_revision(&mut self, pool: Option<&rayon::ThreadPool>) {
        self.run_revision_with_cold_count(pool, COLD_ITEMS_PER_REVISION);
    }

    fn run_revision_with_cold_count(
        &mut self,
        pool: Option<&rayon::ThreadPool>,
        cold_count: usize,
    ) {
        self.advance_revision();

        let cold = new_items(&self.db, &self.tracker, self.next_value, cold_count);
        self.next_value += cold_count;
        let cold_expected = expected_sum(&cold, &self.db);
        match pool {
            Some(pool) => assert_eq!(
                black_box(access_partitioned_concurrently(pool, &self.db, &cold)),
                cold_expected
            ),
            None => access_and_assert(&self.db, &cold, cold_expected),
        }

        // Access the stable working set last so recency-based policies see the
        // same hot/cold distinction as policies that observe reuse by revision.
        self.access_hot(pool);
    }

    fn finish_revision(&mut self) {
        self.advance_revision();
        self.access_hot(None);
    }

    fn access_hot(&mut self, pool: Option<&rayon::ThreadPool>) {
        let executions_before = self.tracker.executions();
        match pool {
            Some(pool) => {
                for actual in access_all_concurrently(pool, &self.db, &self.hot) {
                    assert_eq!(black_box(actual), self.hot_expected);
                }
            }
            None => access_and_assert(&self.db, &self.hot, self.hot_expected),
        }
        self.hot_recomputations += self.tracker.executions() - executions_before;
    }

    fn start_measurement(&mut self) {
        self.hot_recomputations = 0;
        self.measured_dropped = self.tracker.dropped();
        self.measured_executions = self.tracker.executions();
    }

    fn assert_measured_reclamation(&self) {
        let executions = self.tracker.executions() - self.measured_executions;
        let expected_executions = MEASURED_REVISIONS * COLD_ITEMS_PER_REVISION;
        let hot_recomputations = self.hot_recomputations;
        assert!(
            hot_recomputations <= MAX_HOT_RECOMPUTATIONS,
            "expected at most {MAX_HOT_RECOMPUTATIONS} hot-set recomputations, \
             got {hot_recomputations}"
        );

        let dropped = self.tracker.dropped() - self.measured_dropped;
        assert!(
            dropped >= MIN_MEASURED_RECLAIM,
            "expected at least {MIN_MEASURED_RECLAIM} reclaimed values, got {dropped}"
        );
        assert_eq!(
            executions,
            expected_executions + hot_recomputations,
            "one-shot cold values should execute exactly once"
        );
    }
}

fn churn_fixture(pool: Option<&rayon::ThreadPool>) -> ChurnFixture {
    let mut fixture = ChurnFixture::new();
    let dropped_before = fixture.tracker.dropped();

    for _ in 0..MAX_WARMUP_REVISIONS {
        fixture.run_revision(pool);

        let dropped = fixture.tracker.dropped() - dropped_before;
        if dropped >= WARMUP_RECLAIM_TARGET {
            fixture.start_measurement();
            return fixture;
        }
    }

    panic!(
        "eviction did not reclaim {WARMUP_RECLAIM_TARGET} values within \
         {MAX_WARMUP_REVISIONS} revisions"
    )
}

fn dead_value_fixture() -> (ChurnFixture, Vec<Item>) {
    let mut fixture = ChurnFixture::new();
    let dead = new_items(
        &fixture.db,
        &fixture.tracker,
        fixture.next_value,
        DEAD_ITEMS,
    );
    fixture.next_value += DEAD_ITEMS;
    let expected = expected_sum(&dead, &fixture.db);
    access_and_assert(&fixture.db, &dead, expected);
    fixture.access_hot(None);
    fixture.start_measurement();
    (fixture, dead)
}

fn run_quiescent_revisions(fixture: &mut ChurnFixture) {
    for _ in 0..QUIESCENT_REVISIONS {
        fixture.advance_revision();
        fixture.access_hot(None);
    }

    assert!(
        fixture.hot_recomputations <= MAX_QUIESCENT_HOT_RECOMPUTATIONS,
        "expected at most {MAX_QUIESCENT_HOT_RECOMPUTATIONS} hot-set recomputations, got {}",
        fixture.hot_recomputations
    );
    let dropped = fixture.tracker.dropped() - fixture.measured_dropped;
    assert!(
        dropped >= MIN_QUIESCENT_RECLAIM,
        "expected at least {MIN_QUIESCENT_RECLAIM} reclaimed values, got {dropped}"
    );
    black_box((fixture.tracker.live(), fixture.tracker.dropped()));
}

struct BurstDecayFixture {
    fixture: ChurnFixture,
    _burst: Vec<Item>,
}

impl BurstDecayFixture {
    fn new() -> Self {
        let mut fixture = ChurnFixture::new();
        let burst = new_items(
            &fixture.db,
            &fixture.tracker,
            fixture.next_value,
            LARGE_BURST_ITEMS,
        );
        let expected = expected_sum(&burst, &fixture.db);
        access_and_assert(&fixture.db, &burst, expected);
        fixture.access_hot(None);
        fixture.start_measurement();
        Self {
            fixture,
            _burst: burst,
        }
    }

    fn measure(&mut self) {
        for _ in 0..BURST_DECAY_REVISIONS {
            self.fixture.advance_revision();
            self.fixture.access_hot(None);
        }
        assert!(self.fixture.hot_recomputations <= BURST_DECAY_REVISIONS * CHURN_HOT_ITEMS / 50);
        black_box((self.fixture.tracker.live(), self.fixture.tracker.dropped()));
    }
}

fn measure_admission_rate(fixture: &mut ChurnFixture, rate: usize) {
    let mut midpoint_live = 0;
    for revision in 0..ADMISSION_RATE_REVISIONS {
        fixture.run_revision_with_cold_count(None, rate);
        if revision + 1 == ADMISSION_RATE_REVISIONS / 2 {
            midpoint_live = fixture.tracker.live();
        }
    }
    fixture.finish_revision();

    let executions = fixture.tracker.executions() - fixture.measured_executions;
    assert_eq!(
        executions,
        ADMISSION_RATE_REVISIONS * rate + fixture.hot_recomputations
    );
    black_box((
        midpoint_live,
        fixture.tracker.live(),
        fixture.tracker.dropped() - fixture.measured_dropped,
        fixture.hot_recomputations,
    ));
}

struct SparseTableFixture {
    db: salsa::DatabaseImpl,
    clock: Clock,
    tracker: Tracker,
    rows: Vec<Item>,
    expected: usize,
}

impl SparseTableFixture {
    fn new(row_count: usize) -> Self {
        let mut db = salsa::DatabaseImpl::new();
        lru_value::set_lru_capacity(&mut db, CHURN_CAPACITY);
        let clock = Clock::new(&db, 0);
        let tracker = Tracker::default();
        let rows = new_items(&db, &tracker, 0, row_count);
        let expected = expected_sum(&rows[..SPARSE_RESIDENTS], &db);
        access_and_assert(&db, &rows[..SPARSE_RESIDENTS], expected);
        Self {
            db,
            clock,
            tracker,
            rows,
            expected,
        }
    }

    fn measure(&mut self) {
        let executions = self.tracker.executions();
        for revision in 1..=SPARSE_REVISIONS {
            self.clock.set_revision(&mut self.db).to(revision);
            access_and_assert(&self.db, &self.rows[..SPARSE_RESIDENTS], self.expected);
        }
        assert_eq!(self.tracker.executions(), executions);
        black_box((self.rows.len(), self.tracker.live()));
    }
}

struct PhaseChangeFixture {
    db: salsa::DatabaseImpl,
    clock: Clock,
    tracker: Tracker,
    old: Vec<Item>,
    old_expected: usize,
    new: Vec<Item>,
    new_expected: usize,
    revision: usize,
}

impl PhaseChangeFixture {
    fn new() -> Self {
        let mut db = salsa::DatabaseImpl::new();
        lru_value::set_lru_capacity(&mut db, CHURN_CAPACITY);
        let clock = Clock::new(&db, 0);
        let tracker = Tracker::default();
        let old = new_items(&db, &tracker, 0, PHASE_ITEMS);
        let new = new_items(&db, &tracker, PHASE_ITEMS, PHASE_ITEMS);
        let old_expected = expected_sum(&old, &db);
        let new_expected = expected_sum(&new, &db);
        access_and_assert(&db, &old, old_expected);

        let mut fixture = Self {
            db,
            clock,
            tracker,
            old,
            old_expected,
            new,
            new_expected,
            revision: 0,
        };
        for _ in 0..PHASE_WARM_REVISIONS {
            fixture.advance_revision();
            access_and_assert(&fixture.db, &fixture.old, fixture.old_expected);
        }
        fixture
    }

    fn advance_revision(&mut self) {
        self.revision += 1;
        self.clock.set_revision(&mut self.db).to(self.revision);
    }

    fn measure(&mut self) {
        let executions_before = self.tracker.executions();
        for _ in 0..PHASE_MEASURED_REVISIONS {
            self.advance_revision();
            access_and_assert(&self.db, &self.new, self.new_expected);
        }
        let new_executions = self.tracker.executions() - executions_before;

        self.advance_revision();
        let executions_before = self.tracker.executions();
        access_and_assert(&self.db, &self.old, self.old_expected);
        let old_refaults = self.tracker.executions() - executions_before;
        black_box((
            new_executions,
            old_refaults,
            self.tracker.live(),
            self.tracker.dropped(),
        ));
    }
}

struct ScanResistanceFixture {
    fixture: ChurnFixture,
    _scan: Vec<Item>,
}

impl ScanResistanceFixture {
    fn new() -> Self {
        let mut fixture = ChurnFixture::new();
        for _ in 0..SCAN_HOT_WARM_REVISIONS {
            fixture.advance_revision();
            fixture.access_hot(None);
        }

        let scan = new_items(
            &fixture.db,
            &fixture.tracker,
            fixture.next_value,
            SCAN_ITEMS,
        );
        let expected = expected_sum(&scan, &fixture.db);
        access_and_assert(&fixture.db, &scan, expected);
        fixture.start_measurement();

        Self {
            fixture,
            _scan: scan,
        }
    }

    fn measure(&mut self) {
        for _ in 0..SCAN_SETTLE_REVISIONS {
            self.fixture.advance_revision();
        }
        self.fixture.access_hot(None);
        assert!(self.fixture.hot_recomputations <= CHURN_HOT_ITEMS);
        let dropped = self.fixture.tracker.dropped() - self.fixture.measured_dropped;
        assert!(
            dropped >= MIN_SCAN_RECLAIM,
            "expected at least {MIN_SCAN_RECLAIM} reclaimed values, got {dropped}"
        );
        assert!(
            self.fixture.tracker.live() <= CHURN_CAPACITY + CHURN_HOT_ITEMS,
            "scan entries should no longer keep the cache above its configured capacity"
        );
        black_box((
            self.fixture.hot_recomputations,
            self.fixture.tracker.live(),
            self.fixture.tracker.dropped(),
        ));
    }
}

struct SkewedFixture {
    fixture: ChurnFixture,
    traces: Vec<(Vec<Item>, usize)>,
}

impl SkewedFixture {
    fn new() -> Self {
        let mut fixture = ChurnFixture::new();
        let mut items = fixture.hot.clone();
        let remaining = SKEWED_ITEMS - items.len();
        items.extend(new_items(
            &fixture.db,
            &fixture.tracker,
            fixture.next_value,
            remaining,
        ));
        fixture.next_value += remaining;

        let traces = (0..SKEWED_REVISIONS)
            .map(|revision| {
                let accesses = (0..SKEWED_ACCESSES_PER_REVISION)
                    .map(|access| {
                        let random =
                            mix_index((revision * SKEWED_ACCESSES_PER_REVISION + access) as u64);
                        let (start, len) = match random % 10 {
                            0..=5 => (0, CHURN_HOT_ITEMS),
                            6..=8 => (CHURN_HOT_ITEMS, SKEWED_WARM_ITEMS),
                            _ => (
                                CHURN_HOT_ITEMS + SKEWED_WARM_ITEMS,
                                SKEWED_ITEMS - CHURN_HOT_ITEMS - SKEWED_WARM_ITEMS,
                            ),
                        };
                        items[start + mix_index(random) as usize % len]
                    })
                    .collect::<Vec<_>>();
                let expected = expected_sum(&accesses, &fixture.db);
                (accesses, expected)
            })
            .collect();
        fixture.start_measurement();

        Self { fixture, traces }
    }

    fn measure(&mut self) {
        for (trace, expected) in &self.traces {
            self.fixture.advance_revision();
            access_and_assert(&self.fixture.db, trace, *expected);
        }
        self.fixture.advance_revision();
        black_box((
            self.fixture.tracker.executions() - self.fixture.measured_executions,
            self.fixture.tracker.live(),
            self.fixture.tracker.dropped(),
        ));
    }
}

fn mix_index(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

struct CyclicFixture {
    fixture: ChurnFixture,
    rotating: Vec<Item>,
    next_rotating: usize,
}

impl CyclicFixture {
    fn new() -> Self {
        let mut fixture = ChurnFixture::new();
        let rotating = new_items(
            &fixture.db,
            &fixture.tracker,
            fixture.next_value,
            CYCLIC_ITEMS,
        );
        fixture.next_value += CYCLIC_ITEMS;
        fixture.start_measurement();
        Self {
            fixture,
            rotating,
            next_rotating: 0,
        }
    }

    fn next_rotating_range(&mut self) -> std::ops::Range<usize> {
        let start = self.next_rotating;
        self.next_rotating = (self.next_rotating + COLD_ITEMS_PER_REVISION) % self.rotating.len();
        start..start + COLD_ITEMS_PER_REVISION
    }

    fn run_revision(&mut self, pool: Option<&rayon::ThreadPool>) {
        self.fixture.advance_revision();
        let range = self.next_rotating_range();
        let rotating = &self.rotating[range];
        let expected = expected_sum(rotating, &self.fixture.db);
        match pool {
            Some(pool) => assert_eq!(
                black_box(access_partitioned_concurrently(
                    pool,
                    &self.fixture.db,
                    rotating,
                )),
                expected
            ),
            None => access_and_assert(&self.fixture.db, rotating, expected),
        }
        self.fixture.access_hot(pool);
    }

    fn finish(&mut self) {
        self.fixture.finish_revision();
        assert!(
            self.fixture.hot_recomputations <= MAX_CYCLIC_HOT_RECOMPUTATIONS,
            "expected at most {MAX_CYCLIC_HOT_RECOMPUTATIONS} hot-set recomputations, got {}",
            self.fixture.hot_recomputations
        );
        black_box((self.fixture.tracker.live(), self.fixture.tracker.dropped()));
    }
}

fn measure_churn(fixture: &mut ChurnFixture, pool: Option<&rayon::ThreadPool>) {
    for _ in 0..MEASURED_REVISIONS {
        fixture.run_revision(pool);
    }
    fixture.finish_revision();
    fixture.assert_measured_reclamation();
}

fn measure_cyclic(fixture: &mut CyclicFixture, pool: Option<&rayon::ThreadPool>) {
    for _ in 0..CYCLIC_REVISIONS {
        fixture.run_revision(pool);
    }
    fixture.finish();
}

fn lru(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("LRU");

    group.bench_function(BenchmarkId::new("hot_cache_hits", HOT_ITEMS), |b| {
        let (db, items, expected) = hot_cache();

        b.iter(|| access_plain_and_assert(black_box(&db), black_box(&items), expected));
    });

    group.bench_function(
        BenchmarkId::new(
            "concurrent_hot_cache_hits",
            format!("{CONCURRENT_WORKERS}x{HOT_ITEMS}"),
        ),
        |b| {
            let (db, items, expected) = hot_cache();
            let pool = thread_pool();

            b.iter(|| {
                for actual in
                    access_all_plain_concurrently(&pool, black_box(&db), black_box(&items))
                {
                    assert_eq!(black_box(actual), expected);
                }
            });
        },
    );

    group.bench_function(
        BenchmarkId::new("first_collection_after_hot_hits", HOT_ITEMS),
        |b| {
            b.iter_batched_ref(
                hot_cache_for_collection,
                |(db, items, expected, tracker, executions)| {
                    access_and_assert(black_box(db), black_box(items), *expected);
                    assert_eq!(tracker.executions(), *executions);
                    trigger_collection_without_reclaiming(db, tracker);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "concurrent_first_collection_after_hot_hits",
            format!("{CONCURRENT_WORKERS}x{HOT_ITEMS}"),
        ),
        |b| {
            let pool = thread_pool();

            b.iter_batched_ref(
                hot_cache_for_collection,
                |(db, items, expected, tracker, executions)| {
                    for actual in access_all_concurrently(&pool, black_box(db), black_box(items)) {
                        assert_eq!(black_box(actual), *expected);
                    }
                    assert_eq!(tracker.executions(), *executions);
                    trigger_collection_without_reclaiming(db, tracker);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "concurrent_steady_state_hot_hits_and_collection",
            format!("{CONCURRENT_WORKERS}x{HOT_ITEMS}"),
        ),
        |b| {
            let pool = thread_pool();

            b.iter_batched_ref(
                hot_cache_for_steady_collection,
                |(db, clock, revision, items, expected, tracker, executions)| {
                    let live = tracker.live();
                    let dropped = tracker.dropped();
                    clock.set_revision(db).to(*revision);
                    assert_eq!(tracker.live(), live);
                    assert_eq!(tracker.dropped(), dropped);
                    for actual in access_all_concurrently(&pool, black_box(db), black_box(items)) {
                        assert_eq!(black_box(actual), *expected);
                    }
                    assert_eq!(tracker.executions(), *executions);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(BenchmarkId::new("disabled_capacity_hits", HOT_ITEMS), |b| {
        let mut db = salsa::DatabaseImpl::new();
        plain_lru_value::set_lru_capacity(&mut db, 0);
        let tracker = Tracker::default();
        let items = new_items(&db, &tracker, 0, HOT_ITEMS);
        let expected = expected_sum(&items, &db);
        access_plain_and_assert(&db, &items, expected);

        b.iter(|| access_plain_and_assert(black_box(&db), black_box(&items), expected));
    });

    group.bench_function(
        BenchmarkId::new(
            "dead_values_during_quiescent_revisions",
            format!("{DEAD_ITEMS}dead+{CHURN_HOT_ITEMS}hot/{QUIESCENT_REVISIONS}rev"),
        ),
        |b| {
            b.iter_batched_ref(
                dead_value_fixture,
                |(fixture, dead)| {
                    black_box(dead);
                    run_quiescent_revisions(fixture);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "large_burst_decay",
            format!("{LARGE_BURST_ITEMS}dead/{BURST_DECAY_REVISIONS}rev"),
        ),
        |b| {
            b.iter_batched_ref(
                BurstDecayFixture::new,
                BurstDecayFixture::measure,
                BatchSize::LargeInput,
            );
        },
    );

    for row_count in SPARSE_TABLE_ROWS {
        group.bench_function(
            BenchmarkId::new(
                "large_table_sparse_residency",
                format!("{row_count}rows/{SPARSE_RESIDENTS}resident/{SPARSE_REVISIONS}rev"),
            ),
            |b| {
                b.iter_batched_ref(
                    || SparseTableFixture::new(row_count),
                    SparseTableFixture::measure,
                    BatchSize::LargeInput,
                );
            },
        );
    }

    group.bench_function(
        BenchmarkId::new(
            "phase_change",
            format!(
                "{PHASE_ITEMS}old->{PHASE_ITEMS}new/\
                 {PHASE_WARM_REVISIONS}rev-warm+{PHASE_MEASURED_REVISIONS}rev-measured"
            ),
        ),
        |b| {
            b.iter_batched_ref(
                PhaseChangeFixture::new,
                PhaseChangeFixture::measure,
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "scan_resistance",
            format!(
                "{SCAN_ITEMS}scan+{CHURN_HOT_ITEMS}hot/\
                 {SCAN_HOT_WARM_REVISIONS}rev-warm+{SCAN_SETTLE_REVISIONS}rev-settle"
            ),
        ),
        |b| {
            b.iter_batched_ref(
                ScanResistanceFixture::new,
                ScanResistanceFixture::measure,
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "skewed_frequency_working_set",
            format!(
                "{SKEWED_ITEMS}items/{SKEWED_REVISIONS}x{SKEWED_ACCESSES_PER_REVISION}accesses"
            ),
        ),
        |b| {
            b.iter_batched_ref(
                SkewedFixture::new,
                SkewedFixture::measure,
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "one_shot_churn",
            format!("{MEASURED_REVISIONS}x{COLD_ITEMS_PER_REVISION}+{CHURN_HOT_ITEMS}hot"),
        ),
        |b| {
            b.iter_batched_ref(
                || churn_fixture(None),
                |fixture| measure_churn(fixture, None),
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "concurrent_one_shot_churn",
            format!(
                "{CONCURRENT_WORKERS}x{MEASURED_REVISIONS}x{COLD_ITEMS_PER_REVISION}+\
                 {CHURN_HOT_ITEMS}hot"
            ),
        ),
        |b| {
            let pool = thread_pool();

            b.iter_batched_ref(
                || churn_fixture(Some(&pool)),
                |fixture| measure_churn(fixture, Some(&pool)),
                BatchSize::LargeInput,
            );
        },
    );

    for rate in ADMISSION_RATES {
        group.bench_function(
            BenchmarkId::new(
                "continuous_churn_by_rate",
                format!("{rate}x{ADMISSION_RATE_REVISIONS}"),
            ),
            |b| {
                b.iter_batched_ref(
                    || {
                        let mut fixture = ChurnFixture::new();
                        fixture.start_measurement();
                        fixture
                    },
                    |fixture| measure_admission_rate(fixture, rate),
                    BatchSize::LargeInput,
                );
            },
        );
    }

    group.bench_function(
        BenchmarkId::new(
            "cyclic_working_set",
            format!("{CYCLIC_ITEMS}rotating+{CHURN_HOT_ITEMS}hot/{CYCLIC_REVISIONS}rev"),
        ),
        |b| {
            b.iter_batched_ref(
                CyclicFixture::new,
                |fixture| measure_cyclic(fixture, None),
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new(
            "concurrent_cyclic_working_set",
            format!(
                "{CONCURRENT_WORKERS}x{CYCLIC_ITEMS}rotating+{CHURN_HOT_ITEMS}hot/\
                 {CYCLIC_REVISIONS}rev"
            ),
        ),
        |b| {
            let pool = thread_pool();

            b.iter_batched_ref(
                CyclicFixture::new,
                |fixture| measure_cyclic(fixture, Some(&pool)),
                BatchSize::LargeInput,
            );
        },
    );

    group.finish();
}

criterion_group!(benches, lru);
criterion_main!(benches);
