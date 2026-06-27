use std::hint::black_box;
use std::mem::transmute;

use salsa::Setter;

fn main() {
    divan::main();
}

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

#[salsa::tracked]
pub struct Tracked<'db> {
    pub value: usize,
}

#[salsa::tracked]
pub fn make_tracked(db: &dyn salsa::Database, input: Input) -> Tracked<'_> {
    Tracked::new(db, input.text(db).len())
}

mod benches {
    use super::*;

    #[divan::bench(name = "tracked_struct::read_field")]
    fn read_tracked_field(bencher: divan::Bencher) {
        let db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, "hello, world!".to_owned());
        let tracked = make_tracked(&db, input);

        bencher.bench_local(|| tracked.value(black_box(&db)));
    }

    #[divan::bench(
        name = "mutating_inputs::Mutating Inputs::mutating",
        args = [10, 20, 30]
    )]
    fn mutating(bencher: divan::Bencher, n: usize) {
        bencher
            .with_inputs(move || {
                let db = salsa::DatabaseImpl::default();
                let base_string = "hello, world!".to_owned();
                let base_len = base_string.len();

                let string = base_string.clone().repeat(n);
                let new_len = string.len();

                let input = Input::new(black_box(&db), black_box(base_string.clone()));
                let actual_len = length(&db, input);
                assert_eq!(black_box(actual_len), base_len);

                (db, input, string, new_len)
            })
            .bench_local_refs(|&mut (ref mut db, input, ref string, new_len)| {
                input.set_text(black_box(db)).to(black_box(string).clone());
                let actual_len = length(db, input);
                assert_eq!(black_box(actual_len), new_len);
            });
    }

    #[divan::bench(name = "inputs::Mutating Inputs::new[InternedInput]")]
    fn new_interned_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
                let db = salsa::DatabaseImpl::default();
                // Prepopulate ingredients.
                let input =
                    InternedInput::new(black_box(&db), black_box("hello, world!".to_owned()));
                let interned_len = interned_length(black_box(&db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
                db
            })
            .bench_local_refs(|db| {
                let input =
                    InternedInput::new(black_box(db), black_box("hello, world!".to_owned()));
                let interned_len = interned_length(black_box(db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
            });
    }

    #[divan::bench(name = "inputs::Mutating Inputs::amortized[InternedInput]")]
    fn amortized_interned_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
                let db = salsa::DatabaseImpl::default();
                // we can't pass this along otherwise, and the lifetime is generally informational
                let input: InternedInput<'static> =
                    unsafe { transmute(InternedInput::new(&db, "hello, world!".to_owned())) };
                let interned_len = interned_length(black_box(&db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
                (db, input)
            })
            .bench_local_refs(|(db, input)| {
                let interned_len = interned_length(black_box(db), black_box(*input));
                assert_eq!(black_box(interned_len), 13);
            });
    }

    #[divan::bench(name = "inputs::Mutating Inputs::new[Input]")]
    fn new_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
                let db = salsa::DatabaseImpl::default();

                // Prepopulate ingredients.
                let input = Input::new(black_box(&db), black_box("hello, world!".to_owned()));
                let len = length(black_box(&db), black_box(input));
                assert_eq!(black_box(len), 13);

                db
            })
            .bench_local_refs(|db| {
                let input = Input::new(black_box(db), black_box("hello, world!".to_owned()));
                let len = length(black_box(db), black_box(input));
                assert_eq!(black_box(len), 13);
            });
    }

    #[divan::bench(name = "inputs::Mutating Inputs::amortized[Input]")]
    fn amortized_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
                let db = salsa::DatabaseImpl::default();

                let input = Input::new(black_box(&db), black_box("hello, world!".to_owned()));
                let len = length(black_box(&db), black_box(input));
                assert_eq!(black_box(len), 13);

                (db, input)
            })
            .bench_local_refs(|(db, input)| {
                let len = length(black_box(db), black_box(*input));
                assert_eq!(black_box(len), 13);
            });
    }

    #[divan::bench(name = "inputs::Mutating Inputs::cached[Input]")]
    fn cached_input(bencher: divan::Bencher) {
        let db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, "hello, world!".to_owned());

        // Prewarm the memo. Every measured call reads a value that has already been verified in
        // the current revision.
        assert_eq!(length(&db, input), 13);

        bencher.bench_local(|| length(black_box(&db), black_box(input)));
    }

    #[divan::bench(name = "inputs::Mutating Inputs::new[SupertypeInput]")]
    fn new_supertype_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
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
            })
            .bench_local_refs(|db| {
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
            });
    }

    #[divan::bench(name = "inputs::Mutating Inputs::amortized[SupertypeInput]")]
    fn amortized_supertype_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
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
            })
            .bench_local_refs(|(db, input, interned_input)| {
                let len = either_length(black_box(db), black_box(*input));
                assert_eq!(black_box(len), 13);
                let len = either_length(black_box(db), black_box(*interned_input));
                assert_eq!(black_box(len), 13);
            });
    }
}
