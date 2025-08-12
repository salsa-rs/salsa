use std::hint::black_box;
use std::mem::transmute;

use codspeed_criterion_compat::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion,
};
use salsa::Setter;

#[salsa::input]
pub struct Input {
    #[returns(ref)]
    pub text: String,
}

#[salsa::tracked]
#[inline(never)]
pub fn length(db: &dyn salsa::Database, input: Input) -> usize {
    input.text(db).len()
}

#[salsa::interned]
pub struct InternedInput<'db> {
    #[returns(ref)]
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, salsa::Supertype)]
enum SupertypeInput<'db> {
    InternedInput(InternedInput<'db>),
    Input(Input),
}

#[salsa::tracked]
#[inline(never)]
pub fn interned_length<'db>(db: &'db dyn salsa::Database, input: InternedInput<'db>) -> usize {
    input.text(db).len()
}

#[salsa::tracked]
#[inline(never)]
pub fn either_length<'db>(db: &'db dyn salsa::Database, input: SupertypeInput<'db>) -> usize {
    match input {
        SupertypeInput::InternedInput(input) => interned_length(db, input),
        SupertypeInput::Input(input) => length(db, input),
    }
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

                    let input = Input::new(black_box(&db), black_box(base_string.clone()));
                    let actual_len = length(&db, input);
                    assert_eq!(black_box(actual_len), base_len);

                    (db, input, string, new_len)
                },
                |&mut (ref mut db, input, ref string, new_len)| {
                    input.set_text(black_box(db)).to(black_box(string).clone());
                    let actual_len = length(db, input);
                    assert_eq!(black_box(actual_len), new_len);
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
            || {
                let db = salsa::DatabaseImpl::default();
                // Prepopulate ingredients.
                let input =
                    InternedInput::new(black_box(&db), black_box("hello, world!".to_owned()));
                let interned_len = interned_length(black_box(&db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
                db
            },
            |db| {
                let input =
                    InternedInput::new(black_box(db), black_box("hello, world!".to_owned()));
                let interned_len = interned_length(black_box(db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
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
                let interned_len = interned_length(black_box(&db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
                (db, input)
            },
            |&mut (ref db, input)| {
                let interned_len = interned_length(black_box(db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("new", "Input"), |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::default();

                // Prepopulate ingredients.
                let input = Input::new(black_box(&db), black_box("hello, world!".to_owned()));
                let len = length(black_box(&db), black_box(input));
                assert_eq!(black_box(len), 13);

                db
            },
            |db| {
                let input = Input::new(black_box(db), black_box("hello, world!".to_owned()));
                let len = length(black_box(db), black_box(input));
                assert_eq!(black_box(len), 13);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("amortized", "Input"), |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::default();

                let input = Input::new(black_box(&db), black_box("hello, world!".to_owned()));
                let len = length(black_box(&db), black_box(input));
                assert_eq!(black_box(len), 13);

                (db, input)
            },
            |&mut (ref db, input)| {
                let len = length(black_box(db), black_box(input));
                assert_eq!(black_box(len), 13);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("new", "SupertypeInput"), |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::default();

                // Prepopulate ingredients.
                let input = SupertypeInput::Input(Input::new(
                    black_box(&db),
                    black_box("hello, world!".to_owned()),
                ));
                let interned_input = SupertypeInput::InternedInput(InternedInput::new(
                    black_box(&db),
                    black_box("hello, world!".to_owned()),
                ));
                let len = either_length(black_box(&db), black_box(input));
                assert_eq!(black_box(len), 13);
                let len = either_length(black_box(&db), black_box(interned_input));
                assert_eq!(black_box(len), 13);

                db
            },
            |db| {
                let input = SupertypeInput::Input(Input::new(
                    black_box(db),
                    black_box("hello, world!".to_owned()),
                ));
                let interned_input = SupertypeInput::InternedInput(InternedInput::new(
                    black_box(db),
                    black_box("hello, world!".to_owned()),
                ));
                let len = either_length(black_box(db), black_box(input));
                assert_eq!(black_box(len), 13);
                let len = either_length(black_box(db), black_box(interned_input));
                assert_eq!(black_box(len), 13);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function(BenchmarkId::new("amortized", "SupertypeInput"), |b| {
        b.iter_batched_ref(
            || {
                let db = salsa::DatabaseImpl::default();

                let input = SupertypeInput::Input(Input::new(
                    black_box(&db),
                    black_box("hello, world!".to_owned()),
                ));
                let interned_input = SupertypeInput::InternedInput(InternedInput::new(
                    black_box(&db),
                    black_box("hello, world!".to_owned()),
                ));
                // we can't pass this along otherwise, and the lifetime is generally informational
                let interned_input: SupertypeInput<'static> = unsafe { transmute(interned_input) };
                let len = either_length(black_box(&db), black_box(input));
                assert_eq!(black_box(len), 13);
                let len = either_length(black_box(&db), black_box(interned_input));
                assert_eq!(black_box(len), 13);

                (db, input, interned_input)
            },
            |&mut (ref db, input, interned_input)| {
                let len = either_length(black_box(db), black_box(input));
                assert_eq!(black_box(len), 13);
                let len = either_length(black_box(db), black_box(interned_input));
                assert_eq!(black_box(len), 13);
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(benches, mutating_inputs, inputs);
criterion_main!(benches);
