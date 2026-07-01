use std::hint::black_box;

use salsa::Setter;

fn main() {
    divan::main();
}

#[divan::bench(args = [10, 20, 30])]
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
