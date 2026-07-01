use std::hint::black_box;

fn main() {
    divan::main();
}

#[divan::bench]
fn new_interned_input(bencher: divan::Bencher) {
    bencher
        .with_inputs(|| {
            let db = salsa::DatabaseImpl::default();
            // Prepopulate ingredients.
            let input = InternedInput::new(black_box(&db), black_box("hello, world!".to_owned()));
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
