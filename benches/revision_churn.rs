//! A long-lived database that repeatedly creates, updates, and deletes entities.

use std::hint::black_box;

use codspeed_criterion_compat::{
    BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main,
};
use salsa::Setter;

const REVISIONS: usize = 128;
const STABLE_ITEMS: usize = 1_024;
const SMALL_LIVE_SET: usize = 1_536;
const LARGE_LIVE_SET: usize = 2_048;

#[salsa::input]
struct Session {
    generation: usize,
}

#[salsa::interned(revisions = 3)]
struct Symbol<'db> {
    name: usize,
}

#[salsa::tracked]
struct Item<'db> {
    /// Stable items retain their symbol across revisions; volatile items receive a new symbol.
    symbol: Symbol<'db>,

    #[tracked]
    payload: usize,
}

#[salsa::tracked(returns(ref))]
#[inline(never)]
fn lower_items(db: &dyn salsa::Database, session: Session) -> Vec<Item<'_>> {
    let generation = session.generation(db);
    let live_items = if generation % 2 == 0 {
        LARGE_LIVE_SET
    } else {
        SMALL_LIVE_SET
    };

    (0..live_items)
        .map(|index| {
            let stable = index < STABLE_ITEMS;
            let name = if stable {
                index
            } else {
                generation.wrapping_mul(LARGE_LIVE_SET).wrapping_add(index)
            };
            let payload = if stable {
                index
            } else {
                generation.wrapping_add(index)
            };
            Item::new(db, Symbol::new(db, name), payload)
        })
        .collect()
}

#[salsa::tracked]
#[inline(never)]
fn analyze_item<'db>(db: &'db dyn salsa::Database, item: Item<'db>) -> usize {
    item.symbol(db)
        .name(db)
        .wrapping_mul(31)
        .wrapping_add(item.payload(db))
}

#[salsa::tracked]
#[inline(never)]
fn analyze_session(db: &dyn salsa::Database, session: Session) -> usize {
    lower_items(db, session)
        .iter()
        .fold(0usize, |checksum, &item| {
            checksum.wrapping_add(analyze_item(db, item))
        })
}

struct ChurnFixture {
    db: salsa::DatabaseImpl,
    session: Session,
    generation: usize,
}

impl ChurnFixture {
    fn new() -> Self {
        let db = salsa::DatabaseImpl::new();
        let session = Session::new(&db, 0);
        assert_ne!(analyze_session(&db, session), 0);
        Self {
            db,
            session,
            generation: 0,
        }
    }

    fn advance(&mut self) -> usize {
        self.generation += 1;
        self.session
            .set_generation(black_box(&mut self.db))
            .to(black_box(self.generation));
        analyze_session(black_box(&self.db), black_box(self.session))
    }

    fn advance_by(&mut self, revisions: usize) {
        for _ in 0..revisions {
            black_box(self.advance());
        }
    }
}

fn revision_churn(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("Revision churn");

    group.bench_function(
        BenchmarkId::new("session", format!("{REVISIONS}x{LARGE_LIVE_SET}")),
        |b| {
            b.iter_batched_ref(
                ChurnFixture::new,
                |fixture| fixture.advance_by(REVISIONS),
                BatchSize::LargeInput,
            );
        },
    );

    group.bench_function(
        BenchmarkId::new("aged_edit", format!("after_{REVISIONS}_revisions")),
        |b| {
            b.iter_batched_ref(
                || {
                    let mut fixture = ChurnFixture::new();
                    fixture.advance_by(REVISIONS);
                    fixture
                },
                |fixture| {
                    assert_ne!(black_box(fixture.advance()), 0);
                },
                BatchSize::LargeInput,
            );
        },
    );

    group.finish();
}

criterion_group!(benches, revision_churn);
criterion_main!(benches);
