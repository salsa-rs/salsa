use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use salsa::setter::Setter;

trait Db: salsa::Database + salsa::DbWithJar<Jar> {}

#[salsa::jar(db=Db)]
struct Jar(Input, Tracked<'_>, index, root);

#[derive(Default)]
#[salsa::db(Jar)]
struct TestDb {
    storage: salsa::Storage<Self>,
}

impl Db for TestDb {}
impl salsa::Database for TestDb {}

#[salsa::input]
struct Input {
    field: usize,
}

#[salsa::tracked]
struct Tracked<'db> {
    #[id]
    number: usize,
}

#[salsa::tracked(return_ref)]
fn index<'db>(db: &'db dyn Db, input: Input) -> Vec<Tracked<'db>> {
    (0..input.field(db)).map(|i| Tracked::new(db, i)).collect()
}

#[salsa::tracked]
fn root(db: &dyn Db, input: Input) -> usize {
    let index = index(db, input);
    index.len()
}

fn many_tracked_structs(criterion: &mut Criterion) {
    criterion.bench_function("many_tracked_structs", |b| {
        b.iter_batched_ref(
            || {
                let db = TestDb::default();

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

                let result = root(db, *input);

                assert_eq!(result, 1_000);
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, many_tracked_structs);
criterion_main!(benches);
