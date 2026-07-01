use std::hint::black_box;

fn main() {
    divan::main();
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
