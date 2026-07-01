use std::hint::black_box;

#[path = "support/input.rs"]
mod input;
#[path = "support/interned.rs"]
mod interned;
#[path = "support/supertype.rs"]
mod supertype;

use input::Input;
use interned::InternedInput;
use supertype::{SupertypeInput, either_length};

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
