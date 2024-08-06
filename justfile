test:
    cargo test --workspace --all-features --all-targets

miri:
    cargo +nightly miri test --no-fail-fast --all-features

all: test miri