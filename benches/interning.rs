use std::hint::black_box;

fn main() {
    divan::main();
}

const VALUES: u64 = 16_384;

#[salsa::interned(no_lifetime)]
struct InternedValue {
    value: u64,
}

#[divan::bench(name = "interning::Interning::hits")]
fn hits(bencher: divan::Bencher) {
    let db = salsa::DatabaseImpl::new();
    for value in 0..VALUES {
        black_box(InternedValue::new(&db, value));
    }

    bencher.bench_local(|| {
        for value in 0..VALUES {
            black_box(InternedValue::new(black_box(&db), black_box(value)));
        }
    });
}

#[divan::bench(name = "interning::Interning::insert")]
fn insert(bencher: divan::Bencher) {
    bencher
        .with_inputs(salsa::DatabaseImpl::new)
        .bench_local_values(|db| {
            for value in 0..VALUES {
                black_box(InternedValue::new(black_box(&db), black_box(value)));
            }
        });
}
