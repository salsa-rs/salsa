use std::hint::black_box;

#[path = "support/interned.rs"]
mod interned;

use interned::{InternedInput, interned_length};

fn main() {
    divan::main();
}

mod benches {
    use super::*;

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
                // Allocate the payload outside the measured region. Otherwise, changes to the
                // allocator's state from unrelated benchmarks can dominate this benchmark.
                (db, "hello, world!".to_owned())
            })
            .bench_local_refs(|(db, text)| {
                let input = InternedInput::new(black_box(&*db), black_box(std::mem::take(text)));
                let interned_len = interned_length(black_box(&*db), black_box(input));
                assert_eq!(black_box(interned_len), 13);
            });
    }
}
