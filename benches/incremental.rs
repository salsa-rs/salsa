use std::hint::black_box;

use codspeed_criterion_compat::{criterion_group, criterion_main, BatchSize, Criterion};
use salsa::Setter;

#[salsa::input]
struct Input {
    field: usize,
}

#[salsa::tracked]
struct Tracked<'db> {
    number: usize,
}

#[salsa::tracked(returns(ref))]
#[inline(never)]
fn index<'db>(db: &'db dyn salsa::Database, input: Input) -> Vec<Tracked<'db>> {
    (0..input.field(db)).map(|i| Tracked::new(db, i)).collect()
}

#[salsa::tracked]
#[inline(never)]
fn root(db: &dyn salsa::Database, input: Input) -> usize {
    let index = index(db, input);
    index.len()
}

fn many_tracked_structs(criterion: &mut Criterion) {
    criterion.bench_function("many_tracked_structs", |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::new();

                let input = Input::new(black_box(&db), black_box(1_000));
                let input2 = Input::new(black_box(&db), black_box(1));

                // prewarm cache
                let root1 = root(black_box(&db), black_box(input));
                assert_eq!(black_box(root1), 1_000);
                let root2 = root(black_box(&db), black_box(input2));
                assert_eq!(black_box(root2), 1);

                (db, input, input2)
            },
            |(db, input, input2)| {
                // Make a change, but fetch the result for the other input
                input2.set_field(black_box(db)).to(black_box(2));

                let result = root(black_box(db), *black_box(input));

                assert_eq!(black_box(result), 1_000);
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, many_tracked_structs);
criterion_main!(benches);
