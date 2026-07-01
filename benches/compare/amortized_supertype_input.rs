use std::hint::black_box;
use std::mem::transmute;

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
                // We can't pass this along otherwise, and the lifetime is generally informational.
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
