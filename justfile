test:
    cargo test --workspace --all-features --all-targets --no-fail-fast

miri:
    cargo +nightly miri test --no-fail-fast --all-features

all: test miri