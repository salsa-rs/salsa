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

#[salsa::tracked(return_ref)]
fn index<'db>(db: &'db dyn salsa::Database, input: Input) -> salsa::Result<Vec<Tracked<'db>>> {
    (0..input.field(db)?).map(|i| Tracked::new(db, i)).collect()
}

#[salsa::tracked]
fn root(db: &dyn salsa::Database, input: Input) -> salsa::Result<usize> {
    let index = index(db, input)?;
    Ok(index.len())
}

fn many_tracked_structs(criterion: &mut Criterion) {
    criterion.bench_function("many_tracked_structs", |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::new();

                let input = Input::new(&db, 1_000);
                let input2 = Input::new(&db, 1);

                // prewarm cache
                let _ = root(&db, input);
                let _ = root(&db, input2);

                (db, input, input2)
            },
            |(db, input, input2)| {
                // Make a change, but fetch the result for the other input
                input2.set_field(db).to(2);

                let result = root(db, *input).unwrap();

                assert_eq!(result, 1_000);
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, many_tracked_structs);
criterion_main!(benches);
