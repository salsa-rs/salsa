use std::hint::black_box;
use std::mem::transmute;

#[path = "support/interned.rs"]
mod interned;

use interned::{InternedInput, interned_length};

fn main() {
    divan::main();
}

mod benches {
    use super::*;

    #[divan::bench(name = "inputs::Mutating Inputs::amortized[InternedInput]")]
    fn amortized_interned_input(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| {
                let db = salsa::DatabaseImpl::default();
                // We can't pass this along otherwise, and the lifetime is generally informational.
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
}
