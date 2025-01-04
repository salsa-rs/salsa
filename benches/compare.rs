use std::mem::transmute;

use codspeed_criterion_compat::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion,
};
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

    for n in &[10, 20, 30] {
        group.bench_function(BenchmarkId::new("mutating", n), |b| {
            b.iter_batched_ref(
                || {
                    let db = salsa::DatabaseImpl::default();
                    let base_string = "hello, world!".to_owned();
                    let base_len = base_string.len();

                    let string = base_string.clone().repeat(*n);
                    let new_len = string.len();
                    (db, base_string, base_len, string, new_len)
                },
                |&mut (ref mut db, ref base_string, base_len, ref string, new_len)| {
                    let input = Input::new(db, base_string.clone());
                    let actual_len = length(db, input);
                    assert_eq!(base_len, actual_len);

                    input.set_text(db).to(string.clone());
                    let actual_len = length(db, input);
                    assert_eq!(new_len, actual_len);
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

fn inputs(c: &mut Criterion) {
    let mut group: codspeed_criterion_compat::BenchmarkGroup<
        codspeed_criterion_compat::measurement::WallTime,
    > = c.benchmark_group("Mutating Inputs");

    group.bench_function(BenchmarkId::new("new", "InternedInput"), |b| {
        b.iter_batched_ref(
            salsa::DatabaseImpl::default,
            |db| {
                let input: InternedInput = InternedInput::new(db, "hello, world!".to_owned());
                interned_length(db, input);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("amortized", "InternedInput"), |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::default();
                // we can't pass this along otherwise, and the lifetime is generally informational
                let input: InternedInput<'static> =
                    unsafe { transmute(InternedInput::new(&db, "hello, world!".to_owned())) };
                let _ = interned_length(&db, input);
                (db, input)
            },
            |&mut (ref db, input)| {
                interned_length(db, input);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("new", "Input"), |b| {
        b.iter_batched_ref(
            salsa::DatabaseImpl::default,
            |db| {
                let input = Input::new(db, "hello, world!".to_owned());
                length(db, input);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("amortized", "Input"), |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::default();
                let input = Input::new(&db, "hello, world!".to_owned());
                let _ = length(&db, input);
                (db, input)
            },
            |&mut (ref db, input)| {
                length(db, input);
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(benches, mutating_inputs, inputs);
criterion_main!(benches);
