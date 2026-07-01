use std::hint::black_box;

fn main() {
    divan::main();
}

mod benches {
    use super::*;

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
}

#[salsa::input]
pub struct Input {
    #[returns(ref)]
    pub text: String,
}

#[salsa::tracked(returns(copy))]
#[inline(never)]
pub fn length(db: &dyn salsa::Database, input: Input) -> usize {
    input.text(db).len()
}

#[salsa::interned]
pub struct InternedInput<'db> {
    #[returns(ref)]
    pub text: String,
}

#[salsa::tracked(returns(copy))]
#[inline(never)]
pub fn interned_length<'db>(db: &'db dyn salsa::Database, input: InternedInput<'db>) -> usize {
    input.text(db).len()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, salsa::Supertype)]
enum SupertypeInput<'db> {
    InternedInput(InternedInput<'db>),
    Input(Input),
}

#[salsa::tracked(returns(copy))]
#[inline(never)]
pub fn either_length<'db>(db: &'db dyn salsa::Database, input: SupertypeInput<'db>) -> usize {
    match input {
        SupertypeInput::InternedInput(input) => interned_length(db, input),
        SupertypeInput::Input(input) => length(db, input),
    }
}
