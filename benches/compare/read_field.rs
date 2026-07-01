use std::hint::black_box;

fn main() {
    divan::main();
}

#[divan::bench]
fn read_tracked_field(bencher: divan::Bencher) {
    let db = salsa::DatabaseImpl::default();
    let input = Input::new(&db, "hello, world!".to_owned());
    let tracked = make_tracked(&db, input);

    bencher.bench_local(|| tracked.value(black_box(&db)));
}

#[salsa::input]
pub struct Input {
    #[returns(ref)]
    pub text: String,
}

#[salsa::tracked]
pub struct Tracked<'db> {
    #[returns(copy)]
    pub value: usize,
}

#[salsa::tracked(returns(copy))]
pub fn make_tracked(db: &dyn salsa::Database, input: Input) -> Tracked<'_> {
    Tracked::new(db, input.text(db).len())
}
