test:
    cargo test --workspace --all-targets --no-fail-fast

miri:
    cargo +nightly miri test --no-fail-fast

shuttle:
    cargo nextest run --features shuttle --test parallel

all: test miri
