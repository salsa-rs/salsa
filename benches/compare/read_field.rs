use std::hint::black_box;

#[path = "support/input.rs"]
mod input;
#[path = "support/tracked.rs"]
mod tracked;

use input::Input;
use tracked::make_tracked;

fn main() {
    divan::main();
}

mod benches {
    use super::*;

    #[divan::bench(name = "tracked_struct::read_field")]
    fn read_tracked_field(bencher: divan::Bencher) {
        let db = salsa::DatabaseImpl::default();
        let input = Input::new(&db, "hello, world!".to_owned());
        let tracked = make_tracked(&db, input);

        bencher.bench_local(|| tracked.value(black_box(&db)));
    }
}
