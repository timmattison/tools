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
while [[ $# -gt 0 ]]; do
    case "$1" in
        --go)
            run_rust=false
            shift
            ;;
        --rust)
            run_go=false
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [--go] [--rust]"
            echo ""
            echo "Options:"
            echo "  --go    Run only Go tests"
            echo "  --rust  Run only Rust tests"
            echo ""
            echo "If no options are specified, all tests are run."
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information."
            exit 1
            ;;
    esac
done

exit_code=0

if [ "$run_go" = true ]; then
    echo "========================================="
    echo "Running Go tests..."
    echo "========================================="
    if go test ./... -v; then
        echo ""
        echo "[PASS] Go tests passed"
    else
        echo ""
        echo "[FAIL] Go tests failed"
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
        echo "[PASS] Rust tests passed"
    else
        echo ""
        echo "[FAIL] Rust tests failed"
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
