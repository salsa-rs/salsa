test:
    cargo test --workspace --all-features --all-targets --no-fail-fast

miri:
    cargo +nightly miri test --no-fail-fast --all-features

loom:
    RUSTFLAGS="--cfg loom" cargo check --workspace

all: test miri
