use std::hint::black_box;

#[derive(PartialEq, Clone, Copy)]
pub(crate) struct Value(pub(crate) u64);

// Each policy needs its own input. Salsa allocates a memo slot per registered
// query even when a benchmark never calls that query.
#[salsa::input]
pub(crate) struct NoEviction {
    #[returns(copy)]
    value: u64,
}

#[salsa::tracked(returns(copy))]
fn no_eviction_value(db: &dyn salsa::Database, item: NoEviction) -> Value {
    compute_value(item.value(db))
}

#[salsa::input]
pub(crate) struct Lru {
    #[returns(copy)]
    value: u64,
}

#[salsa::tracked(returns(copy), lru = 4096)]
fn lru_value(db: &dyn salsa::Database, item: Lru) -> Value {
    compute_value(item.value(db))
}

pub(crate) trait Item: Copy + Send + Sync {
    fn new(db: &dyn salsa::Database, value: u64) -> Self;
    fn set_capacity(db: &mut dyn salsa::Database, capacity: usize);
    fn fetch(self, db: &dyn salsa::Database) -> Value;
}

impl Item for NoEviction {
    fn new(db: &dyn salsa::Database, value: u64) -> Self {
        Self::new(db, value)
    }

    fn set_capacity(_db: &mut dyn salsa::Database, _capacity: usize) {}

    #[inline]
    fn fetch(self, db: &dyn salsa::Database) -> Value {
        no_eviction_value(db, self)
    }
}

impl Item for Lru {
    fn new(db: &dyn salsa::Database, value: u64) -> Self {
        Self::new(db, value)
    }

    fn set_capacity(db: &mut dyn salsa::Database, capacity: usize) {
        lru_value::set_lru_capacity(db, capacity);
    }

    #[inline]
    fn fetch(self, db: &dyn salsa::Database) -> Value {
        lru_value(db, self)
    }
}

#[cfg(feature = "salsa_unstable")]
#[salsa::input]
pub(crate) struct Sieve {
    #[returns(copy)]
    value: u64,
}

#[cfg(feature = "salsa_unstable")]
// Setup sets the capacity. Avoid allocating SIEVE queues for the other policies.
#[salsa::tracked(returns(copy), eviction(policy = sieve, capacity = 0))]
fn sieve_value(db: &dyn salsa::Database, item: Sieve) -> Value {
    compute_value(item.value(db))
}

#[cfg(feature = "salsa_unstable")]
impl Item for Sieve {
    fn new(db: &dyn salsa::Database, value: u64) -> Self {
        Self::new(db, value)
    }

    fn set_capacity(db: &mut dyn salsa::Database, capacity: usize) {
        sieve_value::set_eviction_capacity(db, capacity);
    }

    #[inline]
    fn fetch(self, db: &dyn salsa::Database) -> Value {
        sieve_value(db, self)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Policy {
    NoEviction,
    Lru,
    #[cfg(feature = "salsa_unstable")]
    Sieve,
}

pub(crate) const POLICIES: &[Policy] = &[
    Policy::NoEviction,
    Policy::Lru,
    #[cfg(feature = "salsa_unstable")]
    Policy::Sieve,
];

// Dispatch once per benchmark so the timed loop calls the tracked function directly.
pub(crate) trait Benchmark {
    fn run<I: Item>(self, bencher: divan::Bencher);
}

impl Policy {
    pub(crate) fn bench(self, bencher: divan::Bencher, workload: impl Benchmark) {
        match self {
            Self::NoEviction => workload.run::<NoEviction>(bencher),
            Self::Lru => workload.run::<Lru>(bencher),
            #[cfg(feature = "salsa_unstable")]
            Self::Sieve => workload.run::<Sieve>(bencher),
        }
    }
}

pub(crate) fn setup<I: Item>(capacity: usize, count: usize) -> (salsa::DatabaseImpl, Vec<I>) {
    let mut db = salsa::DatabaseImpl::new();
    I::set_capacity(&mut db, capacity);
    let items = (0..count).map(|value| I::new(&db, value as u64)).collect();
    (db, items)
}

pub(crate) fn access_all<I: Item>(
    db: &dyn salsa::Database,
    items: impl IntoIterator<Item = I>,
) -> Value {
    let mut sum = 0u64;
    for item in items {
        sum = sum.wrapping_add(black_box(item).fetch(black_box(db)).0);
    }
    Value(sum)
}

pub(crate) fn prewarm<I: Item>(db: &dyn salsa::Database, items: &[I]) {
    black_box(access_all(db, items.iter().copied()));
}

fn compute_value(_value: u64) -> Value {
    const FIBONACCI_STEPS: u32 = 128;

    let mut previous = 0u64;
    let mut current = 1u64;

    for _ in 0..black_box(FIBONACCI_STEPS) {
        (previous, current) = (current, previous.wrapping_add(current));
    }

    Value(previous)
}
