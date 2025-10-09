#!/bin/bash
# Script to run benchmarks with Valgrind/Cachegrind and compare instruction counts
#
# Usage: ./bench_valgrind.sh <benchmark_name> <output_dir>

set -e

BENCH_NAME="${1:-dataflow}"
OUTPUT_DIR="${2:-valgrind_results}"
BENCH_BINARY=""

echo "Building benchmark: $BENCH_NAME"
cargo build --release --bench "$BENCH_NAME"

# Find the benchmark binary
BENCH_BINARY=$(find target/release/deps -name "$BENCH_NAME-*" -type f -executable | head -1)

if [ -z "$BENCH_BINARY" ]; then
    echo "Error: Could not find benchmark binary for $BENCH_NAME"
    exit 1
fi

echo "Found benchmark binary: $BENCH_BINARY"
echo "Running with Valgrind/Cachegrind..."

mkdir -p "$OUTPUT_DIR"
CACHEGRIND_OUT="$OUTPUT_DIR/cachegrind.out"

# Run with Cachegrind
valgrind --tool=cachegrind \
    --cachegrind-out-file="$CACHEGRIND_OUT" \
    --cache-sim=yes \
    --branch-sim=yes \
    "$BENCH_BINARY" --bench --profile-time=1

echo ""
echo "Cachegrind output saved to: $CACHEGRIND_OUT"
echo ""
echo "Summary:"
cg_annotate "$CACHEGRIND_OUT" | head -50

echo ""
echo "To compare with another run, use:"
echo "  cg_diff $CACHEGRIND_OUT <other_cachegrind.out> | cg_annotate --auto=yes -"
