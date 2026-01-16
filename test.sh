#!/bin/bash
# Run all tests for Go and Rust programs in this repository
#
# Usage:
#   ./test.sh           # Run all tests
#   ./test.sh --go      # Run only Go tests
#   ./test.sh --rust    # Run only Rust tests

set -e

cd "$(dirname "$0")"

run_go=true
run_rust=true

# Parse arguments
if [ "$1" = "--go" ]; then
    run_rust=false
elif [ "$1" = "--rust" ]; then
    run_go=false
fi

exit_code=0

if [ "$run_go" = true ]; then
    echo "========================================="
    echo "Running Go tests..."
    echo "========================================="
    if go test ./... -v; then
        echo ""
        echo "✓ Go tests passed"
    else
        echo ""
        echo "✗ Go tests failed"
        exit_code=1
    fi
    echo ""
fi

if [ "$run_rust" = true ]; then
    echo "========================================="
    echo "Running Rust tests..."
    echo "========================================="
    if cargo test --workspace; then
        echo ""
        echo "✓ Rust tests passed"
    else
        echo ""
        echo "✗ Rust tests failed"
        exit_code=1
    fi
    echo ""
fi

if [ $exit_code -eq 0 ]; then
    echo "========================================="
    echo "All tests passed!"
    echo "========================================="
else
    echo "========================================="
    echo "Some tests failed!"
    echo "========================================="
fi

exit $exit_code
