#!/bin/bash
# Script to compare benchmark performance between two git branches using Valgrind/Cachegrind
#
# Usage: ./compare_branches_valgrind.sh <benchmark_name> <branch1> <branch2>

set -e

BENCH_NAME="${1:-dataflow}"
BRANCH1="${2:-master}"
BRANCH2="${3:-fixpoint-scc-sync-table}"
RESULTS_DIR="valgrind_comparison_$(date +%Y%m%d_%H%M%S)"

mkdir -p "$RESULTS_DIR"

echo "========================================="
echo "Benchmark Comparison with Valgrind"
echo "========================================="
echo "Benchmark: $BENCH_NAME"
echo "Branch 1:  $BRANCH1"
echo "Branch 2:  $BRANCH2"
echo "Results:   $RESULTS_DIR"
echo "========================================="
echo ""

# Function to run benchmark on a specific branch
run_benchmark() {
    local branch=$1
    local output_file=$2

    echo "Building $BENCH_NAME on branch $branch..."
    git checkout "$branch" -q
    cargo build --release --bench "$BENCH_NAME" 2>&1 | grep -E "(Compiling salsa|Finished)" || true

    # Find the benchmark binary
    local bench_binary=$(find target/release/deps -name "$BENCH_NAME-*" -type f -executable | head -1)

    if [ -z "$bench_binary" ]; then
        echo "Error: Could not find benchmark binary for $BENCH_NAME on $branch"
        exit 1
    fi

    echo "Running with Valgrind/Cachegrind on $branch..."
    valgrind --tool=cachegrind \
        --cachegrind-out-file="$output_file" \
        --cache-sim=yes \
        --branch-sim=yes \
        "$bench_binary" --bench --profile-time=1 2>&1 | tail -5

    echo "Completed run for $branch"
    echo ""
}

# Save current branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)

# Run on both branches
run_benchmark "$BRANCH1" "$RESULTS_DIR/${BRANCH1}.cachegrind.out"
run_benchmark "$BRANCH2" "$RESULTS_DIR/${BRANCH2}.cachegrind.out"

# Return to original branch
git checkout "$CURRENT_BRANCH" -q

echo "========================================="
echo "Results Summary"
echo "========================================="
echo ""

echo "Branch 1 ($BRANCH1):"
cg_annotate "$RESULTS_DIR/${BRANCH1}.cachegrind.out" | head -30
echo ""

echo "Branch 2 ($BRANCH2):"
cg_annotate "$RESULTS_DIR/${BRANCH2}.cachegrind.out" | head -30
echo ""

echo "========================================="
echo "Difference ($BRANCH2 - $BRANCH1):"
echo "========================================="
cg_diff "$RESULTS_DIR/${BRANCH1}.cachegrind.out" "$RESULTS_DIR/${BRANCH2}.cachegrind.out" | cg_annotate --auto=yes - | head -50

echo ""
echo "========================================="
echo "Full results saved to: $RESULTS_DIR"
echo "To view detailed comparison:"
echo "  cg_diff $RESULTS_DIR/${BRANCH1}.cachegrind.out $RESULTS_DIR/${BRANCH2}.cachegrind.out | cg_annotate --auto=yes -"
echo "========================================="
