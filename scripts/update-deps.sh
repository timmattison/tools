#!/bin/bash
# Update all Rust and Go dependencies to their latest versions
#
# Usage:
#   ./scripts/update-deps.sh              # Update all deps (Rust + Go)
#   ./scripts/update-deps.sh --rust-only  # Update only Rust deps
#   ./scripts/update-deps.sh --go-only    # Update only Go deps
#   ./scripts/update-deps.sh --lock-only  # Update Cargo.lock without changing Cargo.toml
#
# Prerequisites:
#   - cargo-edit: cargo install cargo-edit (for `cargo upgrade`)

set -e

cd "$(dirname "$0")/.."

UPDATE_RUST=true
UPDATE_GO=true
LOCK_ONLY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --rust-only)
            UPDATE_GO=false
            shift
            ;;
        --go-only)
            UPDATE_RUST=false
            shift
            ;;
        --lock-only)
            LOCK_ONLY=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [--rust-only] [--go-only] [--lock-only]"
            echo ""
            echo "Options:"
            echo "  --rust-only   Update only Rust dependencies"
            echo "  --go-only     Update only Go dependencies"
            echo "  --lock-only   Only update Cargo.lock (don't bump Cargo.toml constraints)"
            echo "  -h, --help    Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Run '$0 --help' for usage."
            exit 1
            ;;
    esac
done

if [[ "$UPDATE_RUST" == true ]]; then
    echo "=== Updating Rust dependencies ==="

    if [[ "$LOCK_ONLY" == true ]]; then
        echo "Updating Cargo.lock (within existing semver bounds)..."
        cargo update
    else
        if ! command -v cargo-upgrade &>/dev/null; then
            echo "Error: cargo-edit is required for upgrading Cargo.toml constraints."
            echo "Install it with: cargo install cargo-edit"
            echo "Or run with --lock-only to just update Cargo.lock."
            exit 1
        fi

        echo "Upgrading workspace dependency constraints in Cargo.toml..."
        cargo upgrade --workspace

        echo "Updating Cargo.lock..."
        cargo update
    fi

    echo "Building Rust workspace..."
    cargo build --release

    echo "Running Rust tests..."
    cargo test

    echo "[PASS] Rust dependencies updated successfully."
    echo ""
fi

if [[ "$UPDATE_GO" == true ]]; then
    echo "=== Updating Go dependencies ==="

    echo "Updating Go modules..."
    go get -u ./...

    echo "Tidying go.mod..."
    go mod tidy

    echo "Building Go tools..."
    go build ./...

    echo "Running Go tests..."
    go test ./...

    echo "[PASS] Go dependencies updated successfully."
    echo ""
fi

echo "=== All dependency updates complete ==="
echo ""
echo "Review changes with: git diff"
echo "Commit with: git add -A && git commit -m 'chore: update dependencies'"
