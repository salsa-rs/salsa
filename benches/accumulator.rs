use std::hint::black_box;

use codspeed_criterion_compat::{criterion_group, criterion_main, BatchSize, Criterion};
use salsa::Accumulator;

#[salsa::input]
struct Input {
    expressions: usize,
}

#[allow(dead_code)]
#[salsa::accumulator]
struct Diagnostic(String);

#[salsa::interned]
struct Expression<'db> {
    number: usize,
}

#[salsa::tracked]
#[inline(never)]
fn root<'db>(db: &'db dyn salsa::Database, input: Input) -> Vec<usize> {
    (0..input.expressions(db))
        .map(|i| infer_expression(db, Expression::new(db, i)))
        .collect()
}

#[salsa::tracked]
#[inline(never)]
fn infer_expression<'db>(db: &'db dyn salsa::Database, expression: Expression<'db>) -> usize {
    let number = expression.number(db);

    if number % 10 == 0 {
        Diagnostic(format!("Number is {number}")).accumulate(db);
    }

    if number != 0 && number % 2 == 0 {
        let sub_expression = Expression::new(db, number / 2);
        let _ = infer_expression(db, sub_expression);
    }

    number
}

fn accumulator(criterion: &mut Criterion) {
    criterion.bench_function("accumulator", |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::new();

                let input = Input::new(black_box(&db), black_box(10_000));

                // Pre-warm
                let result = root(black_box(&db), black_box(input));
                assert!(!black_box(result).is_empty());

                (db, input)
            },
            |(db, input)| {
                // Measure the cost of collecting accumulators ignoring the cost of running the
                // query itself.
                let diagnostics = root::accumulated::<Diagnostic>(black_box(db), *black_box(input));

                assert_eq!(black_box(diagnostics).len(), 1000);
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, accumulator);
criterion_main!(benches);
