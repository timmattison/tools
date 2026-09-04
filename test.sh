#!/bin/bash
# Run the test suite for this repository.
#
# Usage:
#   ./test.sh           # Run all tests

set -e

cd "$(dirname "$0")"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            echo "Usage: $0"
            echo ""
            echo "Runs the Rust workspace test suite."
            echo ""
            echo "Options:"
            echo "  -h, --help  Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information."
            exit 1
            ;;
    esac
done

echo "========================================="
echo "Running Rust tests..."
echo "========================================="

if cargo test --workspace; then
    echo ""
    echo "========================================="
    echo "All tests passed!"
    echo "========================================="
else
    echo ""
    echo "========================================="
    echo "Some tests failed!"
    echo "========================================="
    exit 1
fi
