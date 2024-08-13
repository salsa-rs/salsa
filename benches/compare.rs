use codspeed_criterion_compat::{criterion_group, criterion_main, BenchmarkId, Criterion};
use salsa::Setter;

#[salsa::input]
pub struct Input {
    pub text: String,
}

#[salsa::tracked]
pub fn length(db: &dyn salsa::Database, input: Input) -> usize {
    input.text(db).len()
}

#[salsa::interned]
pub struct InternedInput<'db> {
    pub text: String,
}

#[salsa::tracked]
pub fn interned_length<'db>(db: &'db dyn salsa::Database, input: InternedInput<'db>) -> usize {
    input.text(db).len()
}

fn mutating_inputs(c: &mut Criterion) {
    let mut group: codspeed_criterion_compat::BenchmarkGroup<
        codspeed_criterion_compat::measurement::WallTime,
    > = c.benchmark_group("Mutating Inputs");

    let mut db = salsa::DatabaseImpl::default();

    for n in &[10, 20, 30] {
        let base_string = "hello, world!".to_owned();
        let base_len = base_string.len();

        let string = base_string.clone().repeat(*n);
        let new_len = string.len();

        group.bench_function(BenchmarkId::new("mutating", n), |b| {
            b.iter(|| {
                let input = Input::new(&db, base_string.clone());
                let actual_len = length(&db, input);
                assert_eq!(base_len, actual_len);

                input.set_text(&mut db).to(string.clone());
                let actual_len = length(&db, input);
                assert_eq!(new_len, actual_len);
            })
        });
    }

    group.finish();
}

fn inputs(c: &mut Criterion) {
    let mut group: codspeed_criterion_compat::BenchmarkGroup<
        codspeed_criterion_compat::measurement::WallTime,
    > = c.benchmark_group("Mutating Inputs");

    let db = salsa::DatabaseImpl::default();

    group.bench_function(BenchmarkId::new("new", "InternedInput"), |b| {
        b.iter(|| {
            let input: InternedInput = InternedInput::new(&db, "hello, world!".to_owned());
            interned_length(&db, input);
        })
    });

    group.bench_function(BenchmarkId::new("amortized", "InternedInput"), |b| {
        let input = InternedInput::new(&db, "hello, world!".to_owned());
        let _ = interned_length(&db, input);

        b.iter(|| interned_length(&db, input));
    });

    group.bench_function(BenchmarkId::new("new", "Input"), |b| {
        b.iter(|| {
            let input = Input::new(&db, "hello, world!".to_owned());
            length(&db, input);
        })
    });

    group.bench_function(BenchmarkId::new("amortized", "Input"), |b| {
        let input = Input::new(&db, "hello, world!".to_owned());
        let _ = length(&db, input);

        b.iter(|| length(&db, input));
    });

    group.finish();
}

criterion_group!(benches, mutating_inputs, inputs);
criterion_main!(benches);
