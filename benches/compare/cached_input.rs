use std::hint::black_box;

#[path = "support/input.rs"]
mod input;

use input::{Input, length};

fn main() {
    divan::main();
}

mod benches {
    use super::*;

    #[divan::bench(name = "inputs::Mutating Inputs::cached[Input]")]
    fn cached_input(bencher: divan::Bencher) {
        let db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, "hello, world!".to_owned());

        // Prewarm the memo. Every measured call reads a value that has already been verified in
        // the current revision.
        assert_eq!(length(&db, input), 13);

        bencher.bench_local(|| length(black_box(&db), black_box(input)));
    }
}
