use std::hint::black_box;

#[path = "support/input.rs"]
mod input;

use input::{Input, length};

fn main() {
    divan::main();
}

mod benches {
    use super::*;

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
}
